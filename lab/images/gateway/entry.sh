#!/bin/sh
# NAT gateway for the leak lab: the client's "home router". Sits on the real network (10.77.0.1)
# and the internet network (10.88.0.1) and masquerades the clients, so 10.88.0.1 is the client's
# real ISP identity: any lab server that logs it has caught a leak.
set -eu
# Docker gives this container a default route via the real bridge (10.77.0.254, the host). Drop
# it: anything not addressed to a lab network must die here, never reach the real internet.
ip route del default 2>/dev/null || true
iptables -t nat -A POSTROUTING -s 10.77.0.0/24 ! -d 10.77.0.0/24 -j MASQUERADE
echo "gateway: up"
exec sleep infinity
