# Leak lab network-tools image (nyx-lab/net): Debian with userspace WireGuard (wireguard-go, so
# no kernel module is needed), wg tools, iptables, iproute2 and tcpdump.
# Used by three services with different entry scripts: the WireGuard exits (wg-exit/entry.sh),
# the NAT gateway (gateway/entry.sh) and the per-client tcpdump sidecars.
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        wireguard-go wireguard-tools iptables iproute2 tcpdump \
    && rm -rf /var/lib/apt/lists/*
COPY wg-exit/entry.sh /usr/local/bin/wg-exit-entry
COPY gateway/entry.sh /usr/local/bin/gateway-entry
RUN chmod +x /usr/local/bin/wg-exit-entry /usr/local/bin/gateway-entry
