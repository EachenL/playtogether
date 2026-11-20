#!/usr/bin/env python3
"""jt-webd — WebSocket ⇄ UDP 桥，让浏览器能加入 playTogether 房间。

浏览器 ↔ 本进程（WebSocket，二进制帧）：
    48 kHz / 单声道 / 16-bit PCM（little-endian），每帧 480 采样 = 10 ms = 960 字节
本进程 ↔ jt-server（UDP）：
    "JT01" + client_id(u32) + seq(u32) + n_samples(u32) + i16 PCM
    "JTPN" + timestamp(u64) 用于测 RTT

关键点：每个浏览器连接都用自己的一个 UDP socket（独立源端口），
所以 jt-server 会把每个网页用户都当成一个独立乐手，桌面端和网页端可以同房间。
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import socket
import struct
import time

import websockets

try:  # websockets >= 13 的新 asyncio 实现
    from websockets.asyncio.server import serve as ws_serve
except ImportError:  # pragma: no cover - 老版本兜底
    from websockets import serve as ws_serve

LISTEN_HOST = os.environ.get("JT_WEB_HOST", "127.0.0.1")
LISTEN_PORT = int(os.environ.get("JT_WEB_PORT", "9100"))
SERVER_HOST = os.environ.get("JT_SERVER_HOST", "127.0.0.1")
SERVER_PORT = int(os.environ.get("JT_SERVER_PORT", "9000"))
SERVER_ADDR = (SERVER_HOST, SERVER_PORT)

FRAME_SAMPLES = 480          # 10 ms @ 48 kHz
FRAME_BYTES = FRAME_SAMPLES * 2
MAGIC = b"JT01"
PING_MAGIC = b"JTPN"

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s jt-webd %(message)s",
)
log = logging.getLogger("jt-webd")


class Session:
    """一个浏览器连接 = 一个乐手 = 一个 UDP 源端口。"""

    __slots__ = (
        "ws", "sock", "loop", "seq", "tx", "rx", "rtt_ms", "ping_sent", "buf", "closed",
    )

    def __init__(self, ws: websockets.WebSocketServerProtocol):
        self.ws = ws
        self.loop = asyncio.get_running_loop()
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("0.0.0.0", 0))
        self.sock.setblocking(False)
        self.seq = 0
        self.tx = 0
        self.rx = 0
        self.rtt_ms: float | None = None
        self.ping_sent: dict[int, float] = {}
        self.buf = bytearray()
        self.closed = False

    # ---- 出站：PCM → JT01 → UDP ----
    def _encode(self, payload: bytes) -> bytes:
        self.seq = (self.seq + 1) & 0xFFFFFFFF
        header = MAGIC + struct.pack("<III", 0, self.seq, FRAME_SAMPLES)
        return header + payload

    async def send_ping(self) -> None:
        ts = int(time.time() * 1000)
        self.ping_sent[ts] = time.monotonic()
        if len(self.ping_sent) > 64:
            self.ping_sent.clear()
        try:
            await self.loop.sock_sendto(self.sock, PING_MAGIC + struct.pack("<Q", ts), SERVER_ADDR)
        except OSError:
            pass

    async def pump_ws(self) -> None:
        """浏览器 → 服务器。"""
        async for msg in self.ws:
            if isinstance(msg, (bytes, bytearray)):
                self.buf += msg
                while len(self.buf) >= FRAME_BYTES:
                    frame = bytes(self.buf[:FRAME_BYTES])
                    del self.buf[:FRAME_BYTES]
                    try:
                        await self.loop.sock_sendto(self.sock, self._encode(frame), SERVER_ADDR)
                        self.tx += 1
                    except OSError:
                        return
            else:
                try:
                    data = json.loads(msg)
                except (ValueError, TypeError):
                    continue
                if data.get("type") == "ping":
                    # 应用层 ping：让浏览器自己算出「浏览器 ↔ 服务器」的 RTT
                    try:
                        await self.ws.send(json.dumps({
                            "type": "pong",
                            "ts": data.get("ts"),
                            "server_rtt_ms": None if self.rtt_ms is None else round(self.rtt_ms, 2),
                        }))
                    except Exception:
                        return
                    await self.send_ping()

    async def pump_udp(self) -> None:
        """服务器 → 浏览器。"""
        while not self.closed:
            try:
                data, _addr = await self.loop.sock_recvfrom(self.sock, 65536)
            except OSError:
                return

            if data[:4] == PING_MAGIC and len(data) == 12:
                ts = struct.unpack("<Q", data[4:12])[0]
                sent = self.ping_sent.pop(ts, None)
                if sent is not None:
                    self.rtt_ms = (time.monotonic() - sent) * 1000.0
                continue

            if data[:4] == MAGIC and len(data) >= 16:
                n = struct.unpack("<I", data[12:16])[0]
                payload = data[16:16 + n * 2]
                if not payload:
                    continue
                self.rx += 1
                try:
                    await self.ws.send(payload)
                except Exception:
                    return

    async def pump_status(self) -> None:
        """每秒推一条状态给页面。"""
        while not self.closed:
            await asyncio.sleep(1.0)
            try:
                await self.ws.send(json.dumps({
                    "type": "status",
                    "rtt_ms": None if self.rtt_ms is None else round(self.rtt_ms, 2),
                    "tx": self.tx,
                    "rx": self.rx,
                    "frame_ms": FRAME_SAMPLES / 48.0,
                }))
            except Exception:
                return

    async def ping_loop(self) -> None:
        while not self.closed:
            await self.send_ping()
            await asyncio.sleep(1.0)


async def handler(ws) -> None:
    peer = getattr(ws, "remote_address", None)
    session = Session(ws)
    log.info("join peer=%s local_udp_port=%s", peer, session.sock.getsockname()[1])
    tasks = [
        asyncio.create_task(session.pump_ws()),
        asyncio.create_task(session.pump_udp()),
        asyncio.create_task(session.pump_status()),
        asyncio.create_task(session.ping_loop()),
    ]
    try:
        done, _pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        for task in done:
            exc = task.exception()
            if exc:
                log.info("session ended peer=%s reason=%r", peer, exc)
    except Exception as exc:  # pragma: no cover
        log.warning("handler error peer=%s: %r", peer, exc)
    finally:
        session.closed = True
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        try:
            session.sock.close()
        except OSError:
            pass
        log.info("leave peer=%s tx=%d rx=%d", peer, session.tx, session.rx)


async def main() -> None:
    log.info("bridging ws://%s:%d  <->  udp %s:%d", LISTEN_HOST, LISTEN_PORT, SERVER_HOST, SERVER_PORT)
    async with ws_serve(
        handler,
        LISTEN_HOST,
        LISTEN_PORT,
        max_size=1 << 20,
        ping_interval=20,
        ping_timeout=20,
        compression=None,   # 音频不压缩，省一点 CPU 和延迟
    ):
        await asyncio.Future()


if __name__ == "__main__":
    asyncio.run(main())
