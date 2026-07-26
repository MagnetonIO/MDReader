import { Container, getContainer } from '@cloudflare/containers';
import { acceptsPublicRequest } from './routes';

export interface Env {
  CATDB_DEMO: DurableObjectNamespace<CatDbDemoContainer>;
}

const MAX_BODY_BYTES = 64 * 1024;

export class CatDbDemoContainer extends Container<Env> {
  override defaultPort = 8080;
  override sleepAfter = '10m';
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (!acceptsPublicRequest(request)) {
      return Response.json(
        { error: 'not_found', message: 'This endpoint is not available.' },
        { status: 404 },
      );
    }
    const length = Number(request.headers.get('content-length') ?? '0');
    if (!Number.isSafeInteger(length) || length > MAX_BODY_BYTES) {
      return Response.json(
        { error: 'request_too_large', message: 'The request exceeds the demo limit.' },
        { status: 413 },
      );
    }
    return getContainer(env.CATDB_DEMO, 'immutable-demo-v1').fetch(request);
  },
} satisfies ExportedHandler<Env>;
