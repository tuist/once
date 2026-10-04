FROM alpine:3.23.3@sha256:25109184c71bdad752c8312a8623239686a9a2071e8825f20acb8f2198c3f659 AS base
WORKDIR /app
COPY message.txt /app/message.txt

FROM base AS processed
RUN cp message.txt processed.txt

FROM base
COPY --from=processed /app/processed.txt /app/processed.txt
CMD ["cat", "/app/processed.txt"]
