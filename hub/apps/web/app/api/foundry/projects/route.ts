import { getApi } from "@/lib/api";

/** Foundry hub: create or link the Porter project for a site (see docs/superpowers/specs/2026-11-26-foundry-integration-design.md). */
export const POST = (req: Request) => getApi().linkFoundry(req);
