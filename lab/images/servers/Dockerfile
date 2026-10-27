# Leak lab servers image (nyx-lab/servers): tracker, observer, dns and canary are small Python
# scripts sharing one base; compose picks the script per service (`python <name>.py`).
FROM python:3.12-slim
WORKDIR /srv
COPY *.py ./
ENV PYTHONUNBUFFERED=1
