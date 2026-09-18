import js from "@eslint/js";
import tseslint from "typescript-eslint";
import stylistic from "@stylistic/eslint-plugin";
import globals from "globals";
import { defineConfig } from "eslint/config";

export default defineConfig([
  {
    ignores: ["**/.vscode-test", "**/dist", "**/out", "**/.vscode-extension-samples", "**/vscode-extension-samples"],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  ...tseslint.configs.stylistic,
  {
    plugins: {
      "@stylistic": stylistic,
    },
    rules: {
      'curly': 'warn',
      '@stylistic/semi': ['warn', 'always'],
    }
  },
  {
    files: ["**/*.{js,mjs,cjs,mts}"],
    languageOptions: {
      globals: globals.node,
    },
  },
]);
