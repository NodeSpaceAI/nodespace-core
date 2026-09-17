import js from '@eslint/js';
import ts from '@typescript-eslint/eslint-plugin';
import tsParser from '@typescript-eslint/parser';
import svelte from 'eslint-plugin-svelte';
import svelteParser from 'svelte-eslint-parser';
import oxlint from 'eslint-plugin-oxlint';
import unicorn from 'eslint-plugin-unicorn';
import enforceNodeType from './eslint-rules/enforce-nodetype.js';

// Common ESLint rule configuration to avoid duplication
const commonUnusedVarsRule = ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }];

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
        HTMLInputElement: 'readonly',
        performance: 'readonly',
        setTimeout: 'readonly',
        clearTimeout: 'readonly',
        requestAnimationFrame: 'readonly',
        MouseEvent: 'readonly',
        FocusEvent: 'readonly',
        KeyboardEvent: 'readonly',
        DragEvent: 'readonly',
        ClipboardEvent: 'readonly',
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
        Response: 'readonly',
        fetch: 'readonly',
        crypto: 'readonly',
        CSS: 'readonly',
        ResizeObserver: 'readonly',
        TextEncoder: 'readonly',
        TextDecoder: 'readonly'
      }
    },
    plugins: {
      '@typescript-eslint': ts,
      oxlint,
      unicorn,
      'nodespace': {
        rules: {
          'enforce-nodetype': enforceNodeType
        }
      }
    },
    rules: {
      ...oxlint.configs.recommended.rules,
      ...ts.configs.recommended.rules,
      // Customize rules as needed
      '@typescript-eslint/no-unused-vars': commonUnusedVarsRule,
      '@typescript-eslint/no-explicit-any': 'warn',
      'no-console': process.env.NODE_ENV === 'production' ? 'error' : 'off',
      // Custom NodeSpace rules
      'nodespace/enforce-nodetype': 'warn',
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
      '@typescript-eslint/no-unused-vars': commonUnusedVarsRule,
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
        HTMLInputElement: 'readonly',
        performance: 'readonly',
        setTimeout: 'readonly',
        clearTimeout: 'readonly',
        requestAnimationFrame: 'readonly',
        MouseEvent: 'readonly',
        FocusEvent: 'readonly',
        KeyboardEvent: 'readonly',
        DragEvent: 'readonly',
        ClipboardEvent: 'readonly',
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
        Response: 'readonly',
        BeforeUnloadEvent: 'readonly',
        btoa: 'readonly',
        ResizeObserver: 'readonly',
        TextEncoder: 'readonly',
        TextDecoder: 'readonly',
        // Vite `define` compile-time constant (see vite.config.js)
        __APP_VERSION__: 'readonly',
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
        __dirname: 'readonly', // Node.js CommonJS global (available via Bun/Vitest)
        __filename: 'readonly',
        Buffer: 'readonly', // Node.js Buffer global
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
        HTMLInputElement: 'readonly',
        HTMLSelectElement: 'readonly',
        Element: 'readonly',
        Node: 'readonly',
        Text: 'readonly',
        MouseEvent: 'readonly',
        FocusEvent: 'readonly',
        KeyboardEvent: 'readonly',
        DragEvent: 'readonly',
        DataTransfer: 'readonly',
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
      '@typescript-eslint/no-unused-vars': commonUnusedVarsRule,
      '@typescript-eslint/no-explicit-any': 'warn',
      'no-console': 'off', // Allow console in tests
      // Bare wall-clock sleeps longer than the 500ms persistence debounce are
      // how this suite accumulated ~64s of deliberate sleeping, paid on every
      // push (the pre-push gate is the sole CI here). A number above that
      // threshold is almost always a guess padded around a real constant.
      //
      // The 500 below is the same value as PERSISTENCE_DEBOUNCE_MS in
      // tests/utils/test-constants.ts, duplicated because an ESLint flat config
      // cannot import from the test sources it lints. If the production debounce
      // ever changes, tests/services/persistence-timing-constants.test.ts fails
      // first and loudly — update this threshold in the same commit.
      //
      // Deliberately allows a NUMERIC literal <= 500: short sleeps are cheap
      // and usually genuine yields. A named constant (DEBOUNCED_WRITE_WAIT_MS,
      // CASCADE_SETTLE_TIMEOUT_MS, ...) is always allowed regardless of its
      // value, because the point is that the duration be DERIVED and its
      // origin stated, not that it be small.
      //
      // `:nth-child(2)` pins this to the DELAY argument. Without it the rule
      // also flagged literals in any other position — `setTimeout(fn, 10, 9999)`
      // reported the 9999 callback datum as a sleep, giving advice that made no
      // sense for that line.
      //
      // Deliberately a heuristic ratchet, not a proof. It does NOT catch a
      // computed delay (`400 + 500`), a module-level `const D = 9000`, or a
      // member call (`globalThis.setTimeout(fn, 4000)`). Those are rare here and
      // reaching them needs type-aware analysis; the common literal shape — the
      // one that produced every case this repo actually accumulated — is caught.
      'no-restricted-syntax': ['error', {
        selector: "CallExpression[callee.name='setTimeout'] > Literal.arguments:nth-child(2)[value>500]",
        message:
          'Sleeping >500ms on real time in a test. Prefer vi.waitFor on the condition ' +
          'being awaited, or fake timers (vi.advanceTimersByTimeAsync). If a real sleep ' +
          'is genuinely required, import a named constant from tests/utils/test-constants ' +
          'that is derived from the production value it outwaits.'
      }],
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