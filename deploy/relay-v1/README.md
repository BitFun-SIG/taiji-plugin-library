# Official Relay v1 deployment

The owner guide is [Relay Server](../../src/apps/relay-server/README.md).
Use this independent Compose project for `/v/1.0.0/`. Keep the older Relay
container, paths, image, database, and `/relay` proxy location intact.

Deploy from a committed checkout at `/srv/openbitfun-relay-v1/app`. Set
`RELAY_GIT_COMMIT` to that checkout's verified full commit. Build mobile web
from the same checkout with `pnpm run build:mobile-web` and stage its `dist`
contents into `/srv/openbitfun-relay-v1/static`. Create `data` and `assets`
under that root owned by UID/GID 10001 before starting Compose.

The Linux host network plus explicit `127.0.0.1:19700` listener lets the service
verify the immediate proxy peer before trusting its overwritten forwarded IP.
Do not publish this listener on a public interface. Install the versioned Nginx
location from the owner guide after the container passes its health check.

Published Pages are disabled with an explicit 503 until both isolated public
and sign-in origins are configured. This prevents uploaded content from sharing
the mobile controller's account origin. Enabling Pages requires dedicated
origins, their proxy routes, and the Page isolation verification in the owner
guide; setting an arbitrary origin value alone is insufficient.

Before replacement, back up this version's database and assets and retain the
previous image tag. Roll back only this Compose project and its versioned
location. Never use the legacy relay Compose file to operate this deployment.
