// Minimal config used only so the Better Auth CLI can generate auth-schema.ts.
// The real wiring (providers, hooks, mail) lives in apps/web (Task 2).
import { betterAuth } from "better-auth";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { deviceAuthorization, magicLink, organization } from "better-auth/plugins";

export const auth = betterAuth({
  database: drizzleAdapter({} as never, { provider: "pg" }),
  emailAndPassword: { enabled: true },
  plugins: [organization(), magicLink({ sendMagicLink: async () => {} }), deviceAuthorization()],
});
