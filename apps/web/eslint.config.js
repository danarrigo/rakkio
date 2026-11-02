import js from '@eslint/js'
import eslintConfigPrettier from 'eslint-config-prettier'
import pluginVue from 'eslint-plugin-vue'
import globals from 'globals'
import tseslint from 'typescript-eslint'

export default tseslint.config(
  { ignores: ['dist', 'src/api/schema.d.ts'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...pluginVue.configs['flat/recommended'],
  {
    files: ['**/*.{ts,vue}'],
    languageOptions: {
      globals: globals.browser,
      parserOptions: {
        parser: tseslint.parser,
        extraFileExtensions: ['.vue'],
      },
    },
    rules: {
      'vue/multi-word-component-names': 'off',
      'vue/no-v-html': 'error',
    },
  },
  {
    files: ['api/**/*.ts', 'server/**/*.ts'],
    languageOptions: { globals: { ...globals.node, ...globals.browser } },
  },
  eslintConfigPrettier,
)
