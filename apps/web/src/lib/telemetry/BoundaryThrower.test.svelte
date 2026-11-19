<script lang="ts">
  // Test helper: throws while rendering when `control.fail` is set.
  let { control }: { control: { fail: boolean } } = $props()
  // Read once per mount: "Try again" mounts it afresh.
  function renderOrThrow() {
    if (control.fail) throw new Error('boom for alice@example.com at /reset-password?email=x')
  }
  renderOrThrow()
</script>

<p>Panel content</p>
