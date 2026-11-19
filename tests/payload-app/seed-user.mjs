// Creates (or re-keys) the API-key user on a running app and prints ONLY the key on stdout.
// Usage: pnpm seed-user [base-url]   (default http://localhost:3999)
import { randomUUID } from 'node:crypto'

const base = (process.argv[2] || process.env.PAYLOAD_URL || 'http://localhost:3999').replace(/\/$/, '')
const creds = { email: 'porter-seed@example.test', password: 'porter-seed-password' }

async function call(path, init = {}) {
  const res = await fetch(base + path, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...init.headers },
  })
  const body = await res.json().catch(() => ({}))
  return { ok: res.ok, status: res.status, body }
}

let login = await call('/api/users/login', { method: 'POST', body: JSON.stringify(creds) })
if (!login.ok) {
  const reg = await call('/api/users/first-register', { method: 'POST', body: JSON.stringify(creds) })
  if (!reg.ok) {
    console.error(`first-register failed (${reg.status}): ${JSON.stringify(reg.body)}`)
    process.exit(1)
  }
  login = await call('/api/users/login', { method: 'POST', body: JSON.stringify(creds) })
}
if (!login.ok) {
  console.error(`login failed (${login.status}): ${JSON.stringify(login.body)}`)
  process.exit(1)
}

const key = randomUUID()
const upd = await call(`/api/users/${login.body.user.id}`, {
  method: 'PATCH',
  headers: { Authorization: `JWT ${login.body.token}` },
  body: JSON.stringify({ enableAPIKey: true, apiKey: key }),
})
if (!upd.ok) {
  console.error(`enabling API key failed (${upd.status}): ${JSON.stringify(upd.body)}`)
  process.exit(1)
}
process.stdout.write(key + '\n')
