# Nisaba eBay OAuth broker: no dependencies, so there is nothing to install.
FROM node:22-alpine
WORKDIR /app
COPY server.mjs ./
ENV NODE_ENV=production PORT=8080
USER node
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
  CMD wget -q -O /dev/null http://127.0.0.1:8080/healthz || exit 1
CMD ["node", "server.mjs"]
