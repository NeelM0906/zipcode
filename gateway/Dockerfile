FROM python:3.14.6-slim-bookworm@sha256:4c92ffcde4dd6f1ff72a24518f49fd4990b27134987dfa31a733badde66df9f8

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /gateway
COPY gateway/requirements.txt /gateway/requirements.txt
RUN python -m pip install --no-cache-dir --requirement /gateway/requirements.txt \
    && groupadd --gid 1000 zipcode \
    && useradd --uid 1000 --gid zipcode --no-create-home --shell /usr/sbin/nologin zipcode

USER zipcode
