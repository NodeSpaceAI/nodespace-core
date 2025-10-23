import js from '@eslint/js';
import ts from '@typescript-eslint/eslint-plugin';
import tsParser from '@typescript-eslint/parser';
import svelte from 'eslint-plugin-svelte';
import svelteParser from 'svelte-eslint-parser';
import oxlint from 'eslint-plugin-oxlint';
import unicorn from 'eslint-plugin-unicorn';

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
        global: 'readonly',
        globalThis: 'readonly',
        window: 'readonly',
        Window: 'readonly',
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
        NodeFilter: 'readonly',
        // Additional browser APIs needed
        IntersectionObserver: 'readonly',
        MutationObserver: 'readonly',
        EventListener: 'readonly',
        URL: 'readonly',
        URLSearchParams: 'readonly',
        DOMStringMap: 'readonly',
        setInterval: 'readonly',
        clearInterval: 'readonly',
        process: 'readonly',
        Performance: 'readonly',
        AbortController: 'readonly',
        AbortSignal: 'readonly',
        RequestInit: 'readonly',
        Response: 'readonly'
      }
    },
    plugins: {
      '@typescript-eslint': ts,
      oxlint,
      unicorn
    },
    rules: {
      ...oxlint.configs.recommended.rules,
      ...ts.configs.recommended.rules,
      // Customize rules as needed
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      '@typescript-eslint/no-explicit-any': 'warn',
      'no-console': process.env.NODE_ENV === 'production' ? 'error' : 'off',
      // File naming conventions for TypeScript files
      'unicorn/filename-case': ['error', {
        cases: {
          kebabCase: true
        },
        ignore: [
          // Ignore index files and files with specific patterns
          'index\\.ts$',
          '\\.d\\.ts$',
          '\\.test\\.ts$',
          '\\.spec\\.ts$',
          '\\.command\\.ts$' // Allow kebab-case for command files
        ]
      }]
    }
  },
  {
    files: ['**/*.svelte.ts'],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: 2022,
        sourceType: 'module'
      },
      globals: {
        console: 'readonly',
        global: 'readonly',
        globalThis: 'readonly',
        window: 'readonly',
        Window: 'readonly',
        document: 'readonly',
        localStorage: 'readonly',
        $state: 'readonly',
        $derived: 'readonly',
        $effect: 'readonly',
        $props: 'readonly',
        $bindable: 'readonly',
        $inspect: 'readonly'
      }
    },
    plugins: {
      '@typescript-eslint': ts,
      oxlint,
      unicorn
    },
    rules: {
      ...oxlint.configs.recommended.rules,
      ...ts.configs.recommended.rules,
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      '@typescript-eslint/no-explicit-any': 'warn',
      'no-console': process.env.NODE_ENV === 'production' ? 'error' : 'off',
      // File naming conventions for Svelte TypeScript files (kebab-case)
      'unicorn/filename-case': ['error', {
        cases: {
          kebabCase: true
        }
      }]
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
        global: 'readonly',
        globalThis: 'readonly',
        window: 'readonly',
        Window: 'readonly',
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
        NodeFilter: 'readonly',
        // Additional browser APIs needed
        IntersectionObserver: 'readonly',
        MutationObserver: 'readonly',
        EventListener: 'readonly',
        URL: 'readonly',
        URLSearchParams: 'readonly',
        DOMStringMap: 'readonly',
        setInterval: 'readonly',
        clearInterval: 'readonly',
        process: 'readonly',
        Performance: 'readonly',
        AbortController: 'readonly',
        AbortSignal: 'readonly',
        RequestInit: 'readonly',
        Response: 'readonly'
      }
    },
    plugins: {
      svelte,
      unicorn
    },
    rules: {
      ...svelte.configs.recommended.rules,
      // Svelte-specific rules
      'svelte/no-unused-svelte-ignore': 'error',
      'svelte/no-at-html-tags': 'warn',
      'svelte/valid-compile': 'error',
      // Allow underscore-prefixed parameters in type signatures (type-only parameters)
      'no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      // File naming conventions for Svelte components (kebab-case)
      'unicorn/filename-case': ['error', {
        cases: {
          kebabCase: true
        },
        ignore: [
          // Allow PascalCase for component names that haven't been migrated yet
          'app\\.html$'
        ]
      }]
    }
  },
  {
    files: ['src/tests/**/*.{js,ts}'],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        ecmaVersion: 2022,
        sourceType: 'module'
      },
      globals: {
        console: 'readonly',
        global: 'readonly',
        globalThis: 'readonly',
        window: 'readonly',
        Window: 'readonly',
        document: 'readonly',
        localStorage: 'readonly',
        global: 'readonly', // Node.js global for tests
        globalThis: 'readonly',
        process: 'readonly',
        vi: 'readonly', // Vitest
        describe: 'readonly', // Vitest
        it: 'readonly', // Vitest
        expect: 'readonly', // Vitest
        beforeEach: 'readonly', // Vitest
        afterEach: 'readonly', // Vitest
        beforeAll: 'readonly', // Vitest
        afterAll: 'readonly', // Vitest
        test: 'readonly', // Vitest
        // DOM types for tests
        HTMLElement: 'readonly',
        Element: 'readonly',
        Node: 'readonly',
        Text: 'readonly',
        MouseEvent: 'readonly',
        FocusEvent: 'readonly',
        KeyboardEvent: 'readonly',
        Event: 'readonly',
        InputEvent: 'readonly'
      }
    },
    plugins: {
      '@typescript-eslint': ts,
      oxlint,
      unicorn
    },
    rules: {
      ...oxlint.configs.recommended.rules,
      ...ts.configs.recommended.rules,
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      '@typescript-eslint/no-explicit-any': 'warn',
      'no-console': 'off', // Allow console in tests
      // File naming conventions for test files (kebab-case)
      'unicorn/filename-case': ['error', {
        cases: {
          kebabCase: true
        }
      }]
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