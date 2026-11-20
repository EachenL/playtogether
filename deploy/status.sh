#!/usr/bin/env bash
# playTogether 主站状态采集：每 10 秒写一份 api/status.json
set -uo pipefail

SVC=jt-server
OUT=/var/www/playtogether/api/status.json
mkdir -p "$(dirname "$OUT")"

status="$(systemctl is-active "$SVC" 2>/dev/null || true)"
[ -z "$status" ] && status=unknown

# 用 monotonic 时间算运行时长，避免 systemd 本地时间字符串里的 CST 歧义
mono_entry="$(systemctl show -p ActiveEnterTimestampMonotonic --value "$SVC" 2>/dev/null || true)"
now_mono="$(cut -d'.' -f1 /proc/uptime)"
started_iso=""
uptime_s=0
if [ -n "${mono_entry:-}" ] && [ "$mono_entry" != "0" ] && [ -n "${now_mono:-}" ]; then
  entry_s=$(( mono_entry / 1000000 ))
  uptime_s=$(( now_mono - entry_s ))
  [ "$uptime_s" -lt 0 ] && uptime_s=0
  started_iso="$(date -u -d "@$(( $(date +%s) - uptime_s ))" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || true)"
fi

# 服务端只在收到音频包时打印 "clients: N"，所以只有当最后一行足够新才算数
clients=0
last_line="$(journalctl -u "$SVC" -n 40 -o short-unix --no-pager 2>/dev/null | grep 'clients:' | tail -1 || true)"
if [ -n "${last_line:-}" ]; then
  line_ts="${last_line%% *}"
  ts_int="${line_ts%%.*}"
  n="${last_line##*clients: }"
  n="${n%%[!0-9]*}"
  now_s="$(date +%s)"
  if [ -n "${n:-}" ] && [ -n "${ts_int:-}" ] && [ $(( now_s - ts_int )) -le 12 ]; then
    clients="$n"
  fi
fi

rtt="$(python3 - <<'PY' 2>/dev/null || true
import socket, struct, time
try:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(1.0)
    ts = int(time.time() * 1000)
    t0 = time.perf_counter()
    s.sendto(b"JTPN" + struct.pack("<Q", ts), ("127.0.0.1", 9000))
    data, _ = s.recvfrom(64)
    if len(data) == 12 and data[:4] == b"JTPN":
        print("%.3f" % ((time.perf_counter() - t0) * 1000))
except Exception:
    pass
PY
)"
rtt_json=${rtt:-null}
[ -z "${rtt:-}" ] && rtt_json=null

mem_total="$(awk '/^MemTotal:/{print $2}' /proc/meminfo)"
mem_avail="$(awk '/^MemAvailable:/{print $2}' /proc/meminfo)"
mem_used_mb=$(( (mem_total - mem_avail) / 1024 ))
load1="$(cut -d' ' -f1 /proc/loadavg)"
now_iso="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
node="$(hostname)"

tmp="$OUT.tmp"
cat > "$tmp" <<EOF
{"status":"$status","node":"$node","region":"cn-chengdu","endpoint":"daoway.ai:9000","started_at":"$started_iso","uptime_s":$uptime_s,"clients":$clients,"loopback_rtt_ms":$rtt_json,"load1":$load1,"mem_used_mb":$mem_used_mb,"updated_at":"$now_iso"}
EOF
chmod 644 "$tmp"
mv "$tmp" "$OUT"
