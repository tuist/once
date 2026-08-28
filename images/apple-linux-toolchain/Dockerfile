# syntax=docker/dockerfile:1.7

ARG RUST_IMAGE=rust:1.96.0-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc
ARG SWIFT_IMAGE=swift:6.3.2-noble@sha256:c4336909a71b2e69b884f4078cdf98c0cd081911632cb349ef72abc4cbed69fc

FROM ${RUST_IMAGE} AS rust-tools

ARG ACTOOL_REV=e7a65f308dea3aa25cf885d2ae33a0caec4448bb
ARG APPLE_CODESIGN_VERSION=0.29.0

RUN apt-get update -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        pkg-config \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN git clone https://github.com/viraptor/actool.git /src/actool \
    && git -C /src/actool checkout --detach "${ACTOOL_REV}" \
    && cargo test --manifest-path /src/actool/Cargo.toml --locked --test e2e_compile \
    && cargo build --manifest-path /src/actool/Cargo.toml --locked --release

RUN cargo install --locked --version "${APPLE_CODESIGN_VERSION}" \
        --root /opt/once/rcodesign \
        --no-default-features \
        apple-codesign

COPY plutil /src/plutil

RUN cargo test --manifest-path /src/plutil/Cargo.toml --locked \
    && cargo build --manifest-path /src/plutil/Cargo.toml --locked --release

FROM ${SWIFT_IMAGE}

ARG BUILD_DATE
ARG IBTOOL_REV=e5c4cd02f5dcaf3f8b4ddd193a471bb802e71645
ARG LLVM_VERSION=18
ARG VCS_REF

LABEL org.opencontainers.image.title="Once Apple Linux toolchain" \
      org.opencontainers.image.description="Redistributable Linux-native tools for Once Apple graph actions. Apple platform content is intentionally excluded." \
      org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.source="https://github.com/tuist/once"

RUN apt-get update -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates \
        clang-${LLVM_VERSION} \
        file \
        git \
        lld-${LLVM_VERSION} \
        llvm-${LLVM_VERSION} \
        make \
        pkg-config \
        python3 \
        python3-lxml \
        ruby \
        zip \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    ln -sf /usr/bin/clang-${LLVM_VERSION} /usr/local/bin/clang; \
    ln -sf /usr/bin/clang++-${LLVM_VERSION} /usr/local/bin/clang++; \
    ln -sf /usr/bin/ld.lld-${LLVM_VERSION} /usr/local/bin/ld.lld; \
    ln -sf /usr/bin/ld64.lld-${LLVM_VERSION} /usr/local/bin/ld64.lld; \
    ln -sf /usr/bin/llvm-ar-${LLVM_VERSION} /usr/local/bin/llvm-ar; \
    ln -sf /usr/bin/llvm-libtool-darwin-${LLVM_VERSION} /usr/local/bin/llvm-libtool-darwin; \
    ln -sf /usr/bin/llvm-lipo-${LLVM_VERSION} /usr/local/bin/llvm-lipo; \
    ln -sf /usr/bin/llvm-ranlib-${LLVM_VERSION} /usr/local/bin/llvm-ranlib

RUN git clone https://github.com/viraptor/ibtool.git /opt/once/ibtool \
    && git -C /opt/once/ibtool checkout --detach "${IBTOOL_REV}" \
    && rm -rf /opt/once/ibtool/.git

COPY --from=rust-tools /src/actool/target/release/actool /usr/local/bin/actool
COPY --from=rust-tools /opt/once/rcodesign/bin/rcodesign /usr/local/bin/rcodesign
COPY --from=rust-tools /src/plutil/target/release/plutil /usr/local/bin/plutil
COPY ibtool.py /usr/local/bin/ibtool
COPY NOTICE.md toolchain.json testdata/verification.plist /usr/local/share/once-apple-linux-toolchain/
COPY verify-tools.sh /usr/local/bin/once-apple-linux-toolchain-verify

RUN chmod 0755 /usr/local/bin/ibtool /usr/local/bin/once-apple-linux-toolchain-verify \
    && /usr/local/bin/once-apple-linux-toolchain-verify

WORKDIR /workspace
