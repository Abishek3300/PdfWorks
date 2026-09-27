// The tool workspace is a client-only SPA page: the selected tool comes from the
// `?tool=` query param and all processing happens in the browser (or via the
// gated server API). Prerender the shell; render on the client.
export const prerender = true;
export const ssr = false;
