FROM python:3.14.8-slim-bookworm@sha256:c8137f4c460908c8763f281c8f22c431eb5c538514ba9553fc3a89c06b7cfb88

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /gateway
COPY gateway/requirements.txt /gateway/requirements.txt
RUN python -m pip install --no-cache-dir --requirement /gateway/requirements.txt \
    && groupadd --gid 1000 zipcode \
    && useradd --uid 1000 --gid zipcode --no-create-home --shell /usr/sbin/nologin zipcode

USER zipcode
