FROM python:3.12.14-slim-bookworm@sha256:0f5b26b9518d002b6173fd61daad821fa340635ebfec5bba471013f9ca114579

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1

WORKDIR /gateway
COPY gateway/requirements.txt /gateway/requirements.txt
RUN python -m pip install --no-cache-dir --requirement /gateway/requirements.txt \
    && groupadd --gid 1000 zipcode \
    && useradd --uid 1000 --gid zipcode --no-create-home --shell /usr/sbin/nologin zipcode

USER zipcode
