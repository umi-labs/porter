import postgres from "postgres";

export type Listener = {
  /** Calls `fn` on every NOTIFY on `channel`; resolves once LISTEN is active. Returns the unsubscribe. */
  subscribe(channel: string, fn: () => void): Promise<() => void>;
  close(): Promise<void>;
};

/**
 * Postgres LISTEN for SSE. postgres.js keeps every LISTEN of one client on a single dedicated connection and
 * fans each NOTIFY out to that channel's subscribers (relistening after a reconnect), so a web process holds
 * one connection however many browsers are watching.
 */
export function createListener(url = process.env.DATABASE_URL): Listener {
  if (!url) throw new Error("DATABASE_URL is not set");
  const sql = postgres(url, { max: 1 });
  return {
    async subscribe(channel, fn) {
      const { unlisten } = await sql.listen(channel, () => fn());
      return () => void unlisten().catch(() => {});
    },
    close: () => sql.end(),
  };
}

const g = globalThis as { __porterListener?: Listener };
/** One per process, kept on globalThis so Next dev hot reload doesn't open another. */
export const getListener = () => {
  g.__porterListener ??= createListener();
  return g.__porterListener;
};
