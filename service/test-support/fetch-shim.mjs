const upstream = process.env.MACHA_AI_TEST_UPSTREAM_URL;
if (!upstream) throw new Error("Test upstream URL was not configured.");
const realFetch = globalThis.fetch;
globalThis.fetch = (input, init) => {
  const url = new URL(typeof input === "string" ? input : input.url);
  if (url.hostname.endsWith(".trycloudflare.com")) {
    const destination = new URL(`${url.pathname}${url.search}`, upstream);
    return realFetch(destination, init);
  }
  return realFetch(input, init);
};
