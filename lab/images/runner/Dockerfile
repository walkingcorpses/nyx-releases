# Leak lab runner (nyx-lab/runner): Python 3.12 with dpkt (pcap parsing) and cryptography (X25519
# WireGuard keys), plus the docker CLI to drive the lab containers over the mounted docker.sock.
# Build context is lab/ so the shared bencode helper from images/servers is reused.
FROM docker:29-cli AS dockercli

FROM python:3.12-slim
RUN pip install --no-cache-dir dpkt==1.9.8 cryptography==44.0.0
COPY --from=dockercli /usr/local/bin/docker /usr/local/bin/docker
WORKDIR /runner
COPY images/servers/bencode.py runner/*.py ./
ENV PYTHONUNBUFFERED=1
CMD ["python", "scenarios.py"]
