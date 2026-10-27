#!/bin/sh
# Leak lab client entry. Makes the container look like a PC behind a home router: default route
# via the lab gateway (10.77.0.1) and the "ISP" resolver (lab dns, reached directly), so any
# system DNS or direct connection shows up in the capture and in the servers' logs.
# Then waits for the runner to pre-seed settings.json (the VPN profile already active, as after a
# restart of Nyx) and starts the daemon. The runner writes the seed only after the tcpdump
# sidecar is capturing, so the daemon's first packet is always recorded.
set -eu
ip route replace default via 10.77.0.1
echo "nameserver 10.88.0.23" > /etc/resolv.conf
SEED="/conf/${CLIENT_NAME}.settings.json"
while [ ! -f "$SEED" ]; do sleep 0.2; done
mkdir -p "$NYX_DATA_DIR"
cp "$SEED" "$NYX_DATA_DIR/settings.json"
exec nyx-daemon run
