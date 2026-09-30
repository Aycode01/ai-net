FROM node:20-alpine AS builder

WORKDIR /app

# Copy root and backend package definitions for npm workspace / monorepo support
COPY package*.json ./
COPY backend/package*.json ./backend/

RUN npm install --workspace=backend

COPY tsconfig.json ./
COPY backend/ ./backend/

WORKDIR /app/backend
# Build fails the image if TypeScript compilation fails — no || true suppression.
RUN npm run build

FROM node:20-alpine AS runner

WORKDIR /app/backend

ENV NODE_ENV=production
ENV PORT=3000

# Install production-only dependencies
COPY backend/package*.json ./
RUN npm ci --omit=dev

COPY --from=builder /app/backend/dist ./dist
COPY --from=builder /app/backend/scripts ./scripts

# Run as non-root user
RUN addgroup -S appgroup && adduser -S appuser -G appgroup
USER appuser

EXPOSE 3000

HEALTHCHECK --interval=5s --timeout=3s --retries=5 \
  CMD wget --no-verbose --tries=1 --spider http://localhost:3000/health || exit 1

CMD ["node", "dist/src/index.js"]
