# syntax=docker/dockerfile:1.7

FROM rust:slim-bookworm AS rust-base

ARG TARGETARCH
ARG CODEX_NPM_SPEC=@openai/codex
ARG BRAVE_SEARCH_ASSET_REGEX=
ARG ST_ASSET_REGEX=
ARG SIEVE_LCM_ASSET_REGEX=

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git jq libssl-dev npm pkg-config unzip xz-utils \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY docker/install-repo-tool.sh /usr/local/bin/install-repo-tool

RUN chmod +x /usr/local/bin/install-repo-tool

FROM rust-base AS cargo-chef

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo install cargo-chef --locked

FROM cargo-chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY docs/policy ./docs/policy

RUN cargo chef prepare --recipe-path recipe.json

FROM cargo-chef AS builder

COPY --from=planner /src/recipe.json recipe.json

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo chef cook --locked --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY docs/policy ./docs/policy

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release -p sieve-app \
    && install -Dm755 /src/target/release/sieve-app /tmp/sieve-app

FROM rust-base AS tooling

RUN --mount=type=cache,target=/root/.npm \
    mkdir -p /opt/sieve-tools \
    && npm install --global --prefix /opt/sieve-tools "${CODEX_NPM_SPEC}" \
    && TARGETARCH="${TARGETARCH}" INSTALL_ROOT=/opt/sieve-tools install-repo-tool nick1udwig/brave-search bravesearch "${BRAVE_SEARCH_ASSET_REGEX}" \
    && TARGETARCH="${TARGETARCH}" INSTALL_ROOT=/opt/sieve-tools install-repo-tool nick1udwig/st st "${ST_ASSET_REGEX}" \
    && TARGETARCH="${TARGETARCH}" INSTALL_ROOT=/opt/sieve-tools install-repo-tool nick1udwig/sieve-lcm sieve-lcm-cli "${SIEVE_LCM_ASSET_REGEX}"

FROM node:22-bookworm-slim AS node-runtime

FROM ubuntu:24.04 AS runtime

ARG SIEVE_VERSION=0.0.0-dev
ARG VCS_REF=unknown
ARG BUILD_DATE=unknown
ARG SIEVE_UID=1000
ARG SIEVE_GID=1000

LABEL org.opencontainers.image.title="sieve" \
      org.opencontainers.image.description="Prompt-injection-resistant agent runtime" \
      org.opencontainers.image.source="https://github.com/nick1udwig/sieve" \
      org.opencontainers.image.version="${SIEVE_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.created="${BUILD_DATE}"

ENV LANG=C.UTF-8 \
    HOME=/home/sieve \
    PATH=/opt/sieve-tools/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin \
    SIEVE_HOME=/home/sieve/.sieve \
    SIEVE_POLICY_PATH=/opt/sieve/docs/policy/baseline-policy.toml \
    SIEVE_RUNTIME_CWD=/workspace

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash bubblewrap ca-certificates curl ffmpeg git jq libssl3 python3 strace tini trash-cli \
    && if ! getent group "${SIEVE_GID}" >/dev/null; then groupadd --gid "${SIEVE_GID}" sieve; fi \
    && if ! getent passwd "${SIEVE_UID}" >/dev/null; then useradd --uid "${SIEVE_UID}" --gid "${SIEVE_GID}" --create-home --home-dir /home/sieve --shell /bin/bash sieve; fi \
    && mkdir -p /workspace /home/sieve/.sieve \
    && chown -R "${SIEVE_UID}:${SIEVE_GID}" /workspace /home/sieve \
    && rm -rf /var/lib/apt/lists/*

COPY --from=node-runtime /usr/local/ /usr/local/
COPY --from=builder /tmp/sieve-app /usr/local/bin/sieve-app
COPY --from=tooling /opt/sieve-tools /opt/sieve-tools
COPY .env.example /opt/sieve/.env.example
COPY docs/policy /opt/sieve/docs/policy

VOLUME ["/home/sieve/.sieve"]

WORKDIR /workspace
USER ${SIEVE_UID}:${SIEVE_GID}

ENTRYPOINT ["tini", "--", "sieve-app"]
CMD []
