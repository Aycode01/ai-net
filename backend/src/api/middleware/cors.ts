import cors from 'cors';
import { allowedOrigins } from '../../config';

export function createCorsMiddleware() {
  const origins = allowedOrigins();

  return cors({
    origin: (origin, callback) => {
      if (!origin || origins.includes(origin)) {
        callback(null, true);
      } else {
        callback(new Error('Not allowed by CORS'));
      }
    },
    credentials: true,
    methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'],
    // `Idempotency-Key` is required by the browser client for POST /api/tasks;
    // without it in this allow list the preflight fails and the header can
    // never be sent, making the idempotency middleware unreachable from a
    // browser (see #658).
    allowedHeaders: [
      'Content-Type',
      'Authorization',
      'walletpublickey',
      'x-challenge',
      'x-signature',
      'Idempotency-Key',
    ],
  });
}
