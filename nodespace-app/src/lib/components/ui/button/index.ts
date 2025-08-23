import Root from './button.svelte';

// Import types and variants from separate types file for better TypeScript support
import {
  buttonVariants,
  type ButtonProps,
  type ButtonSize,
  type ButtonVariant
} from './types';

export {
  Root,
  Root as Button,
  buttonVariants,
  type ButtonProps,
  type ButtonSize,
  type ButtonVariant,
  type ButtonProps as Props
};
