# playTogether 网络协议参考

所有整数均为**小端序 (little-endian)**。音频走 UDP，无压缩 16-bit PCM。

## 消息一览

| Magic | 含义 | 方向 | 长度 |
|---|---|---|---|
| `JT01` | 音频帧 | 客户端 → 服务器 / 服务器 → 客户端 | 16 + N×2 |
| `JTPN` | ping（测 RTT） | 双向 | 12 |
| `JTFU` | 房间已满 | 服务器 → 客户端 | 4 |
| `JTWC` | 欢迎（分配 client_id） | 服务器 → 客户端 | 8 |
| `JTNM` | 设置显示名 | 客户端 → 服务器 | 4 + 名字字节 |
| `JTRS` | 花名册（id+名字列表） | 服务器 → 客户端 | 变长 |
| `JTGN` | 设置对某 peer 的增益 | 客户端 → 服务器 | 12 |

## 详细布局

### JT01 音频帧

```
[0..4)    "JT01"
[4..8)    room_id      u32   （4 位房间号，0000–9999）
[8..12)   seq          u32   （序号，防乱序/去重）
[12..16)  num_samples  u32
[16..)    samples      i16 × num_samples  （48kHz 单声道）
```

### JTPN ping

```
[0..4)    "JTPN"
[4..12)   timestamp_ms  u64
```
服务器原样回显，客户端据此算 RTT。

### JTWC 欢迎

```
[0..4)    "JTWC"
[4..8)    client_id  u32
```

### JTNM 设置名字

```
[0..4)    "JTNM"
[4..)     name  UTF-8 字节（≤32）
```

### JTRS 花名册

```
[0..4)    "JTRS"
[4..6)    count  u16
然后每个成员：
  [0..4)    client_id  u32
  [4..6)    name_len   u16
  [6..)     name       UTF-8 字节
```

### JTGN 增益

```
[0..4)    "JTGN"
[4..8)    peer_id  u32
[8..12)   gain     f32   （1.0 = 原音量）
```

## 加一个新消息的约定

1. 在 `core/src/lib.rs` 定义 `MAGIC_XXX` 常量 + `make_xxx` / `parse_xxx`。
2. 服务器侧：新建一个插件用 `parse_xxx` 处理（见 `CONTRIBUTING.md`）。
3. 客户端侧（`engine` / 网页桥 `webdeploy/jt_webd.py`）用 `make_xxx` 发送。
4. 保持「音频 JT01 走热路径、控制消息走插件」的约定，别把新消息塞进音频路径。
