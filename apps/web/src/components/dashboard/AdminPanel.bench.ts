// Render cost of the admin user table at a full page (50 rows), read-only and
// with the per-row role selects super-admins get. API mocked; each iteration
// mounts, waits for the last row, and unmounts. Run with `just bench-web`.
import { test, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import AdminPanel from './AdminPanel.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: { me: vi.fn() },
    admin: { listUsers: vi.fn(), setRole: vi.fn() },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigateReplace: vi.fn(),
  redirect: vi.fn(),
}))

import { api } from '../../lib/api'
import type { AdminUserRow, User, UserRole } from '../../lib/types'

const PAGE = 50
const ROLES: UserRole[] = ['user', 'admin', 'super_admin']

const users: AdminUserRow[] = Array.from({ length: PAGE }, (_, i) => ({
  id: `u${i}`,
  email: `user-${i}@example.com`,
  role: ROLES[i % ROLES.length],
  email_verified: i % 4 !== 0,
  created_at: new Date(Date.UTC(2026, 0, 1 + i, 12)).toISOString(),
}))

function viewer(role: UserRole): User {
  return {
    id: 'viewer',
    email: 'viewer@example.com',
    first_name: '',
    last_name: '',
    email_verified: true,
    has_password: true,
    oauth_providers: [],
    role,
  }
}

// Configured at module scope: Vitest bench mode skips beforeAll hooks.
vi.mocked(api.admin.listUsers).mockResolvedValue({ users, limit: PAGE, offset: 0 })

async function renderTable(role: UserRole) {
  sessionStorage.clear()
  vi.mocked(api.auth.me).mockResolvedValue(viewer(role))
  const screen = await render(AdminPanel)
  const lastEmail = users[PAGE - 1].email
  await vi.waitFor(
    () => {
      if (!screen.container.textContent?.includes(lastEmail)) throw new Error('not rendered')
    },
    { interval: 1, timeout: 2000 }
  )
  await screen.unmount()
}

test('AdminPanel render (50 rows)', async ({ bench }) => {
  await bench.compare(
    bench('admin (read-only)', () => renderTable('admin')),
    bench('super_admin (role selects)', () => renderTable('super_admin')),
    { throws: true }
  )
})
