# Build context is the repository root (the web app needs the Rust `once`
# binary from ../crates to generate the CLI reference at compile time), so all
# app paths below are prefixed with `web/`. Build with:
#
#   docker build -f web/Dockerfile -t once-web .

ARG ELIXIR_VERSION=1.20.0
ARG OTP_VERSION=29.0
ARG DEBIAN_VERSION=bookworm-20260518-slim
ARG RUST_VERSION=1.96.0

ARG BUILDER_IMAGE="hexpm/elixir:${ELIXIR_VERSION}-erlang-${OTP_VERSION}-debian-${DEBIAN_VERSION}"
ARG RUNNER_IMAGE="debian:${DEBIAN_VERSION}"

# ---- Rust stage: build the `once` binary used to generate the CLI reference ----
FROM rust:${RUST_VERSION}-slim-bookworm AS once-builder

# libcap-ng is required to link the `once` binary (via the microsandbox deps).
# libasound2-dev is required to link the ALSA backend of cpal, which rodio
# uses for the optional --sound flag; pkg-config resolves alsa-sys's link
# flags and is not present in the slim Rust base image by default.
RUN apt-get update -y && apt-get install -y libcap-ng-dev libasound2-dev pkg-config \
    && apt-get clean && rm -f /var/lib/apt/lists/*_*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release -p once-cli

# ---- Elixir builder ----
FROM ${BUILDER_IMAGE} AS builder

# chromium renders Open Graph images at build time; libcap-ng0 is the runtime
# library the `once` binary (used to generate the CLI reference) links against.
# libasound2 is required at load time by the same binary because cpal
# dynamically links against ALSA on Linux, even when --sound is not used.
RUN apt-get update -y && apt-get install -y build-essential git chromium libcap-ng0 libasound2 \
    && apt-get clean && rm -f /var/lib/apt/lists/*_*

WORKDIR /app

RUN mix local.hex --force && \
    mix local.rebar --force

ENV MIX_ENV="prod"

COPY web/mix.exs web/mix.lock ./
RUN mix deps.get --only $MIX_ENV
RUN mkdir config

COPY web/config/config.exs web/config/${MIX_ENV}.exs config/
RUN mix deps.compile

COPY web/priv priv
COPY web/lib lib
COPY web/assets assets

# The reference generator (Mix.Tasks.Compile.OnceReference) runs during
# `mix compile`; point it at the binary built in the Rust stage.
COPY --from=once-builder /src/target/release/once /usr/local/bin/once
ENV ONCE_BIN=/usr/local/bin/once

RUN mix compile
RUN mix assets.deploy

COPY web/config/runtime.exs config/
RUN mix release

# ---- Runner ----
FROM ${RUNNER_IMAGE}

RUN apt-get update -y && \
  apt-get install -y libstdc++6 openssl libncurses5 locales ca-certificates \
  && apt-get clean && rm -f /var/lib/apt/lists/*_*

RUN sed -i '/en_US.UTF-8/s/^# //g' /etc/locale.gen && locale-gen

ENV LANG=en_US.UTF-8
ENV LANGUAGE=en_US:en
ENV LC_ALL=en_US.UTF-8
ENV MIX_ENV=prod
ENV PHX_SERVER=true

WORKDIR "/app"
RUN chown nobody /app

COPY --from=builder --chown=nobody:root /app/_build/${MIX_ENV}/rel/once_site ./

USER nobody

CMD ["/app/bin/once_site", "start"]
