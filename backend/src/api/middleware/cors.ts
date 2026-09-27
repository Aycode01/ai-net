import cors from 'cors';
import { allowedOrigins } from '../../config';
import { ForbiddenError } from '../../errors/ForbiddenError';
import { createLogger } from '../../utils/logger';

const logger = createLogger({ module: 'cors' });

export function createCorsMiddleware() {
  const origins = allowedOrigins();

  const corsHandler = cors({
    origin: (origin, callback) => {
      if (!origin || origins.includes(origin)) {
        callback(null, true);
      } else {
        // Rejected origins are a client policy outcome, not a server fault.
        // Log at debug level only so scanners cannot flood error logs with
        // attacker-controlled origin strings.
        logger.debug('CORS origin rejected');
        callback(new ForbiddenError('Not allowed by CORS'));
      }
    },
    credentials: true,
    methods: ['GET', 'POST', 'PUT', 'DELETE', 'OPTIONS'],
    allowedHeaders: ['Content-Type', 'Authorization', 'walletpublickey', 'x-challenge', 'x-signature'],
  });

  // Ensure `Vary: Origin` is present on every path — including rejections —
  // so a cached rejection is never served to a legitimate origin.
  return (
    req: Parameters<typeof corsHandler>[0],
    res: Parameters<typeof corsHandler>[1],
    next: Parameters<typeof corsHandler>[2]
  ): void => {
    const existing = res.getHeader('Vary');
    if (!existing) {
      res.setHeader('Vary', 'Origin');
    } else if (typeof existing === 'string' && !existing.includes('Origin')) {
      res.setHeader('Vary', `${existing}, Origin`);
    } else if (Array.isArray(existing) && !existing.includes('Origin')) {
      res.setHeader('Vary', [...existing, 'Origin']);
    }
    corsHandler(req, res, next);
  };
}
