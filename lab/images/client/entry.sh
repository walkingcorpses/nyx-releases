#!/bin/sh
# Leak lab client entry. Makes the container look like a PC behind a home router: default routes
# (IPv4 and IPv6) via the lab gateway (10.77.0.1 / fd77::1) and the "ISP" resolver (lab dns,
# reached directly), so any system DNS or direct connection shows up in the capture and in the
# servers' logs.
# Then waits for the runner to pre-seed settings.json (the VPN profile already active, as after a
# restart of Nyx) and starts the daemon. The runner writes the seed only after the tcpdump
# sidecar is capturing, so the daemon's first packet is always recorded.
# NET_ADMIN is needed only for the routes: it is dropped (bounding and inheritable sets) before
# the supervisor and the daemon run, so the app under test cannot change the network itself.
set -eu
if [ "${1:-}" != "--supervise" ]; then
    ip route replace default via 10.77.0.1
    ip -6 route replace default via fd77::1
    echo "nameserver 10.88.0.23" > /etc/resolv.conf
    SEED="/conf/${CLIENT_NAME}.settings.json"
    while [ ! -f "$SEED" ]; do sleep 0.2; done
    mkdir -p "$NYX_DATA_DIR"
    cp "$SEED" "$NYX_DATA_DIR/settings.json"
    exec setpriv --bounding-set -net_admin --inh-caps -net_admin "$0" --supervise
fi

# Supervisor (PID 1, without NET_ADMIN). The daemon may exit on purpose: the runner's clean
# `shutdown`, or a crash it simulates with SIGKILL (pid in /run/nyx-daemon.pid). The container
# and its capture namespace stay up; the daemon starts again, on the same data dir, once the
# runner creates /run/nyx-restart.
while true; do
    nyx-daemon run &
    echo $! > /run/nyx-daemon.pid
    code=0
    wait $! || code=$?
    echo "client: nyx-daemon exited ($code); waiting for /run/nyx-restart"
    while [ ! -f /run/nyx-restart ]; do sleep 0.2; done
    rm -f /run/nyx-restart
done
