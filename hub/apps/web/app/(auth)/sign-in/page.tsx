import { SignInForm } from "./sign-in-form";

export const dynamic = "force-dynamic";

export default function SignInPage() {
  const e = process.env;
  return (
    <SignInForm
      foundry={!!(e.FOUNDRY_OIDC_ISSUER && e.FOUNDRY_OIDC_CLIENT_ID && e.FOUNDRY_OIDC_CLIENT_SECRET)}
    />
  );
}
