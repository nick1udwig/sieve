FROM rust:slim-bookworm AS builder

ARG TARGETARCH
ARG CODEX_NPM_SPEC=@openai/codex
ARG BRAVE_SEARCH_ASSET_REGEX=
ARG ST_ASSET_REGEX=
ARG SIEVE_LCM_ASSET_REGEX=

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git jq libssl-dev npm pkg-config unzip xz-utils \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY docs/policy ./docs/policy
COPY docker/install-repo-tool.sh /usr/local/bin/install-repo-tool

RUN chmod +x /usr/local/bin/install-repo-tool \
    && cargo build --locked --release -p sieve-app

RUN mkdir -p /opt/sieve-tools \
    && npm install --global --prefix /opt/sieve-tools "${CODEX_NPM_SPEC}" \
    && TARGETARCH="${TARGETARCH}" INSTALL_ROOT=/opt/sieve-tools install-repo-tool nick1udwig/brave-search bravesearch "${BRAVE_SEARCH_ASSET_REGEX}" \
    && TARGETARCH="${TARGETARCH}" INSTALL_ROOT=/opt/sieve-tools install-repo-tool nick1udwig/st st "${ST_ASSET_REGEX}" \
    && TARGETARCH="${TARGETARCH}" INSTALL_ROOT=/opt/sieve-tools install-repo-tool nick1udwig/sieve-lcm sieve-lcm-cli "${SIEVE_LCM_ASSET_REGEX}"

FROM node:22-bookworm-slim AS node-runtime

FROM debian:bookworm-slim AS runtime

ARG SIEVE_VERSION=0.0.0-dev
ARG VCS_REF=unknown
ARG BUILD_DATE=unknown

LABEL org.opencontainers.image.title="sieve" \
      org.opencontainers.image.description="Prompt-injection-resistant agent runtime" \
      org.opencontainers.image.source="https://github.com/nick1udwig/sieve" \
      org.opencontainers.image.version="${SIEVE_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.created="${BUILD_DATE}"

ENV LANG=C.UTF-8 \
    HOME=/data/home \
    PATH=/opt/sieve-tools/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin \
    SIEVE_HOME=/data/.sieve \
    SIEVE_POLICY_PATH=/opt/sieve/docs/policy/baseline-policy.toml \
    SIEVE_RUNTIME_CWD=/workspace

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash bubblewrap ca-certificates curl ffmpeg git jq libssl3 python3 strace tini trash-cli \
    && rm -rf /var/lib/apt/lists/*

COPY --from=node-runtime /usr/local/ /usr/local/
COPY --from=builder /src/target/release/sieve-app /usr/local/bin/sieve-app
COPY --from=builder /opt/sieve-tools /opt/sieve-tools
COPY .env.example /opt/sieve/.env.example
COPY docs/policy /opt/sieve/docs/policy

RUN mkdir -p /workspace /data/.sieve /data/home

WORKDIR /workspace

ENTRYPOINT ["tini", "--", "sieve-app"]
CMD []
