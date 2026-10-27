# Leak lab client (nyx-lab/client): the nyx-daemon image plus iproute2, so the entry script can
# point the default route at the lab NAT gateway. No capture tools here: tcpdump runs in a sidecar.
FROM nyx-lab/daemon
RUN apt-get update && apt-get install -y --no-install-recommends iproute2 \
    && rm -rf /var/lib/apt/lists/*
COPY entry.sh /usr/local/bin/client-entry
RUN chmod +x /usr/local/bin/client-entry
CMD ["client-entry"]
