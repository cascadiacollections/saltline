# Multi-architecture digests; update deliberately for security fixes.
FROM ghcr.io/astral-sh/uv:0.13.5@sha256:32a7bb39b496babbd997b0a39a4d66c126bdaf5661b302bf6f06abda8deca6ff AS uv
FROM docker.io/library/rust:1.99.0-slim-bookworm@sha256:2c3a22f0a5533ea2dd5a16627bc841228151faa2d4de2644ac9987e4a2f1f2fa
COPY --from=uv /uv /uvx /usr/local/bin/
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git python3 unzip \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 developer \
    && useradd --uid 1000 --gid developer --create-home --shell /bin/bash developer \
    && chown -R developer:developer /usr/local/cargo /usr/local/rustup
# The SSH feature includes this drop-in before its main configuration.
RUN mkdir -p /etc/ssh/sshd_config.d \
    && printf '%s\n' 'PermitRootLogin no' 'PasswordAuthentication no' \
        'KbdInteractiveAuthentication no' 'AllowUsers developer' \
        > /etc/ssh/sshd_config.d/00-saltline.conf
USER developer
ENV UV_LINK_MODE=copy
ENV PATH="/home/developer/.local/bin:${PATH}"
# The runtime is installed once per image, not on every attach.
RUN uv python install 3.15.0 \
    && rustup component add rustfmt clippy
WORKDIR /workspaces/saltline
