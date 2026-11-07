import { expect, test } from '@playwright/test'

const TEST_USER = {
  id: 'test-user',
  email: 'test@example.com',
  first_name: 'Ada',
  last_name: 'Lovelace',
  email_verified: true,
  has_password: true,
  oauth_providers: [],
}

// E2E coverage for the destructive self-service Settings actions — data export,
// schedule-deletion (with re-auth + "DELETE" confirmation), and
// cancel-during-grace. These exercise the real `users` API client + SettingsPanel
// danger-zone flow in the browser.
test.describe('account data export & deletion', () => {
  test.beforeEach(async ({ page }) => {
    await page.route('**/api/v1/auth/me', async (route) => {
      await route.fulfill({ json: TEST_USER })
    })
  })

  test('export downloads a machine-readable copy of personal data', async ({ page }) => {
    let exportCount = 0
    await page.route('**/api/v1/users/me/export', async (route) => {
      exportCount += 1
      await route.fulfill({
        json: {
          id: TEST_USER.id,
          email: TEST_USER.email,
          first_name: TEST_USER.first_name,
          last_name: TEST_USER.last_name,
          email_verified: true,
          has_password: true,
          role: 'user',
          created_at: '2026-01-01T00:00:00Z',
          updated_at: '2026-01-01T00:00:00Z',
          linked_oauth_accounts: [],
          exported_at: '2026-06-20T00:00:00Z',
        },
      })
    })

    await page.goto('/dashboard/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()

    // The export triggers a browser download of the returned JSON blob.
    const downloadPromise = page.waitForEvent('download')
    await page.getByRole('button', { name: 'Export my data' }).click()
    const download = await downloadPromise

    expect(download.suggestedFilename()).toBe(`app-data-export-${TEST_USER.id}.json`)
    expect(exportCount).toBe(1)
    await expect(page.getByText('Your data export has been downloaded.')).toBeVisible()
  })

  test('schedule deletion requires re-auth + DELETE, then can be cancelled in grace', async ({
    page,
  }) => {
    const scheduledAt = '2026-07-04T00:00:00Z'

    let deleteBody: { confirmation?: string; password?: string } | null = null
    await page.route('**/api/v1/users/me', async (route) => {
      if (route.request().method() !== 'DELETE') {
        await route.fallback()
        return
      }
      deleteBody = route.request().postDataJSON()
      await route.fulfill({ json: { deletion_scheduled_at: scheduledAt } })
    })

    let cancelCount = 0
    await page.route('**/api/v1/users/me/cancel-deletion', async (route) => {
      cancelCount += 1
      await route.fulfill({ json: { deletion_scheduled_at: null } })
    })

    await page.goto('/dashboard/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()

    const deleteButton = page.getByRole('button', { name: 'Delete my account' })

    // The confirm button stays disabled until the literal "DELETE" is typed.
    await expect(deleteButton).toBeDisabled()
    await page.getByLabel('Confirm your password').fill('hunter2')
    await page.getByLabel('Type DELETE to confirm').fill('DELETE')
    await expect(deleteButton).toBeEnabled()

    await deleteButton.click()

    // The form flips to the scheduled state with a cancel affordance, and the
    // request carried both the password and the confirmation literal.
    await expect(page.getByText('Account deletion scheduled. You can cancel during the grace period.'))
      .toBeVisible()
    await expect(page.getByText(/scheduled for deletion on/)).toBeVisible()
    expect(deleteBody).toEqual({ confirmation: 'DELETE', password: 'hunter2' })

    // Cancelling during the grace period restores the danger-zone form.
    await page.getByRole('button', { name: 'Cancel deletion' }).click()
    await expect(page.getByText('Account deletion cancelled.')).toBeVisible()
    await expect(page.getByLabel('Type DELETE to confirm')).toBeVisible()
    expect(cancelCount).toBe(1)
  })
})
