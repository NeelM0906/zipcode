FROM python:3.14.7-slim-bookworm@sha256:416f0db2a2b561945630cef9877a7ea0581b27449eb9fd9df42f03e1b74b5b63

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /gateway
COPY gateway/requirements.txt /gateway/requirements.txt
RUN python -m pip install --no-cache-dir --requirement /gateway/requirements.txt \
    && groupadd --gid 1000 zipcode \
    && useradd --uid 1000 --gid zipcode --no-create-home --shell /usr/sbin/nologin zipcode

USER zipcode
