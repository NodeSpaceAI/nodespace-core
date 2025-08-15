import js from '@eslint/js';
import ts from '@typescript-eslint/eslint-plugin';
import tsParser from '@typescript-eslint/parser';
import svelte from 'eslint-plugin-svelte';
import svelteParser from 'svelte-eslint-parser';

export default [
  js.configs.recommended,
  {
    files: ['**/*.{js,ts}'],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: 2022,
        sourceType: 'module'
      },
      globals: {
        console: 'readonly',
        window: 'readonly',
        document: 'readonly',
        localStorage: 'readonly',
        MediaQueryListEvent: 'readonly',
        CustomEvent: 'readonly',
        HTMLStyleElement: 'readonly',
        HTMLTextAreaElement: 'readonly',
        HTMLDivElement: 'readonly',
        HTMLParagraphElement: 'readonly',
        FileList: 'readonly',
        getComputedStyle: 'readonly',
        HTMLElement: 'readonly',
        performance: 'readonly',
        setTimeout: 'readonly',
        clearTimeout: 'readonly',
        requestAnimationFrame: 'readonly',
        MouseEvent: 'readonly',
        FocusEvent: 'readonly',
        KeyboardEvent: 'readonly',
        DragEvent: 'readonly',
        Event: 'readonly',
        InputEvent: 'readonly',
        HTMLSpanElement: 'readonly',
        NodeJS: 'readonly',
        Element: 'readonly',
        Text: 'readonly',
        Node: 'readonly',
        Selection: 'readonly',
        Range: 'readonly',
        NodeFilter: 'readonly'
      }
    },
    plugins: {
      '@typescript-eslint': ts
    },
    rules: {
      ...ts.configs.recommended.rules,
      // Customize rules as needed
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      '@typescript-eslint/no-explicit-any': 'warn',
      'no-console': process.env.NODE_ENV === 'production' ? 'error' : 'off'
    }
  },
  {
    files: ['**/*.svelte'],
    languageOptions: {
      parser: svelteParser,
      parserOptions: {
        parser: tsParser,
        extraFileExtensions: ['.svelte']
      },
      globals: {
        console: 'readonly',
        window: 'readonly',
        document: 'readonly',
        localStorage: 'readonly',
        MediaQueryListEvent: 'readonly',
        CustomEvent: 'readonly',
        HTMLStyleElement: 'readonly',
        HTMLTextAreaElement: 'readonly',
        HTMLDivElement: 'readonly',
        HTMLParagraphElement: 'readonly',
        FileList: 'readonly',
        getComputedStyle: 'readonly',
        HTMLElement: 'readonly',
        performance: 'readonly',
        setTimeout: 'readonly',
        clearTimeout: 'readonly',
        requestAnimationFrame: 'readonly',
        MouseEvent: 'readonly',
        FocusEvent: 'readonly',
        KeyboardEvent: 'readonly',
        DragEvent: 'readonly',
        Event: 'readonly',
        InputEvent: 'readonly',
        HTMLSpanElement: 'readonly',
        NodeJS: 'readonly',
        Element: 'readonly',
        Text: 'readonly',
        Node: 'readonly',
        Selection: 'readonly',
        Range: 'readonly',
        NodeFilter: 'readonly'
      }
    },
    plugins: {
      svelte
    },
    rules: {
      ...svelte.configs.recommended.rules,
      // Svelte-specific rules
      'svelte/no-unused-svelte-ignore': 'error',
      'svelte/no-at-html-tags': 'warn',
      'svelte/valid-compile': 'error',
    }
  },
  {
    // Global ignores
    ignores: [
      'build/',
      '.svelte-kit/',
      'dist/',
      'node_modules/',
      'src-tauri/',
      '*.config.js',
      'vite.config.js'
    ]
  }
];