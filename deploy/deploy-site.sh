#!/usr/bin/env bash
# 在服务器上执行的部署脚本（幂等）
set -euo pipefail

SITE=/var/www/playtogether

echo "== 1. 安装 nginx"
if ! command -v nginx >/dev/null 2>&1; then
  sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq nginx
fi
nginx -v 2>&1 | head -1

echo "== 2. 站点文件"
sudo mkdir -p "$SITE/api"
sudo install -m 644 /tmp/pt-index.html "$SITE/index.html"
sudo chown -R www-data:www-data "$SITE"
ls -la "$SITE"

echo "== 3. 状态采集脚本"
sudo install -m 755 /tmp/pt-status.sh /usr/local/bin/playtogether-status.sh

echo "== 4. systemd 定时器"
sudo tee /etc/systemd/system/playtogether-status.service >/dev/null <<'UNIT'
[Unit]
Description=playTogether site status collector
After=network.target

[Service]
Type=oneshot
ExecStart=/usr/local/bin/playtogether-status.sh
UNIT

sudo tee /etc/systemd/system/playtogether-status.timer >/dev/null <<'UNIT'
[Unit]
Description=Refresh playTogether site status every 10s

[Timer]
OnBootSec=15
OnUnitActiveSec=10
AccuracySec=1s

[Install]
WantedBy=timers.target
UNIT

sudo systemctl daemon-reload
sudo systemctl enable --now playtogether-status.timer >/dev/null 2>&1 || true
sudo systemctl start playtogether-status.service || true

echo "== 5. nginx 配置"
sudo install -m 644 /tmp/pt-nginx.conf /etc/nginx/sites-available/playtogether.conf
sudo ln -sf /etc/nginx/sites-available/playtogether.conf /etc/nginx/sites-enabled/playtogether.conf
sudo rm -f /etc/nginx/sites-enabled/default
sudo nginx -t
sudo systemctl enable --now nginx >/dev/null 2>&1 || true
sudo systemctl reload nginx || sudo systemctl restart nginx

echo "== 6. 本机自检"
sleep 1
echo "-- /healthz:"; curl -sS -m 5 -o /dev/null -w '%{http_code}\n' http://127.0.0.1/healthz
echo "-- index 大小:"; curl -sS -m 5 http://127.0.0.1/ | wc -c
echo "-- status.json:"; cat "$SITE/api/status.json" 2>/dev/null || echo MISSING
echo "-- 端口:"; sudo ss -tlnp | grep -E ':80 ' || true
echo "== DONE"
