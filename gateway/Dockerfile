FROM python:3.15.2-slim-bookworm@sha256:96599df822d0e1c6bb700014a05c7f3c8cabc6e0f7ff3c4f9f69cee033b4b5d9

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /gateway
COPY gateway/requirements.txt /gateway/requirements.txt
RUN python -m pip install --no-cache-dir --requirement /gateway/requirements.txt \
    && groupadd --gid 1000 zipcode \
    && useradd --uid 1000 --gid zipcode --no-create-home --shell /usr/sbin/nologin zipcode

USER zipcode
