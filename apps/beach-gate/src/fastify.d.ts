import 'fastify';
import type { VerifiedAccessToken } from './token-service.js';

declare module 'fastify' {
  interface FastifyRequest {
    accessToken?: VerifiedAccessToken;
  }
}
