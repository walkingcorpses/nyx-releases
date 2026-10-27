#!/bin/sh
# WireGuard exit for the leak lab. Waits for the runner to write this exit's config to
# /conf/<EXIT_NAME>.wg (wg setconf format: PrivateKey/ListenPort + the client peer), brings up a
# userspace wireguard-go interface with tunnel address 10.100.0.1/24, and masquerades tunnel
# traffic out of the container, so lab servers see this exit's 10.88.0.x address.
# Writes /conf/<EXIT_NAME>.ready when serving (the runner waits for it).
set -eu
CONF="/conf/${EXIT_NAME}.wg"
while [ ! -f "$CONF" ]; do sleep 0.5; done
# wireguard-go forks into the background once wg0 exists.
wireguard-go wg0
wg setconf wg0 "$CONF"
ip address add 10.100.0.1/24 dev wg0
ip link set wg0 up
iptables -t nat -A POSTROUTING -s 10.100.0.0/24 ! -o wg0 -j MASQUERADE
touch "/conf/${EXIT_NAME}.ready"
echo "wg-exit ${EXIT_NAME}: up"
exec sleep infinity
