const POST_PATHS = new Set(['/v1/search', '/v1/read', '/v1/related']);
const GET_PATHS = new Set(['/v1/sources', '/v1/documents']);

export function acceptsPublicRequest(request: Request): boolean {
  const url = new URL(request.url);
  if (url.search !== '') return false;
  return (
    (request.method === 'POST' && POST_PATHS.has(url.pathname)) ||
    (request.method === 'GET' && GET_PATHS.has(url.pathname))
  );
}
