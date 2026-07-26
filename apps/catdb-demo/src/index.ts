import { Container, getContainer } from '@cloudflare/containers';
import { forwardPublicRequest } from './bounds';
import { acceptsPublicRequest } from './routes';

export interface Env {
  CATDB_DEMO: DurableObjectNamespace<CatDbDemoContainer>;
}

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
    return forwardPublicRequest(request, (bounded) =>
      getContainer(env.CATDB_DEMO, 'immutable-demo-v1').fetch(bounded),
    );
  },
} satisfies ExportedHandler<Env>;
