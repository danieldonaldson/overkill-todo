import tseslint from 'typescript-eslint'
import svelte from 'eslint-plugin-svelte'

export default tseslint.config(
  ...tseslint.configs.recommended,
  ...svelte.configs.recommended,
  {
    files: ['**/*.svelte'],
    languageOptions: {
      parserOptions: {
        parser: tseslint.parser,
      },
    },
  },
  {
    ignores: ['**/dist/**', '**/.astro/**', '**/node_modules/**', '**/target/**', '**/coverage/**', '**/.claude/**', '**/.agents/**'],
  }
)
