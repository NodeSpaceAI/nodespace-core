/**
 * Button component types
 *
 * The filled variants (`default`, `destructive`) hover to their own opaque
 * `--*-hover` token rather than an alpha fill. See DESIGN.md's Button section:
 * an alpha hover composites against whatever is behind it, so `bg-primary/90`
 * lightens on a white page and darkens on a dark one — the direction flips
 * between themes as a side effect rather than a decision, and contrast against
 * the label falls in both.
 *
 * Two stock shadcn treatments were dropped from `destructive` rather than
 * carried forward, because they only made sense together with that alpha hover:
 *
 * - `dark:bg-destructive/60` made the dark REST state translucent. Its effect
 *   was to lighten the fill toward the page, which is the one thing propping up
 *   the `text-white` below it; against the solid `--destructive` fill white is
 *   2.86:1, far under AA. Keeping a translucent rest under an opaque hover would
 *   also be incoherent. The variant is now solid in both themes.
 * - `text-white` bypassed `--destructive-foreground`, which carries real
 *   per-theme values precisely because white fails on dark-mode fills. Using the
 *   token takes dark destructive from 2.86:1 to 6.95:1 at rest.
 *
 * `filled-button-variants.test.ts` holds both variants to the rule.
 */
import type { WithElementRef } from '$lib/utils.js';
import type { HTMLAnchorAttributes, HTMLButtonAttributes } from 'svelte/elements';
import { type VariantProps, tv } from 'tailwind-variants';

export const buttonVariants = tv({
  base: "aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive inline-flex shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium outline-none transition-all disabled:pointer-events-none disabled:opacity-50 aria-disabled:pointer-events-none aria-disabled:opacity-50 [&_svg:not([class*='size-'])]:size-4 [&_svg]:pointer-events-none [&_svg]:shrink-0",
  variants: {
    variant: {
      default: 'bg-primary text-primary-foreground shadow-xs hover:bg-primary-hover',
      destructive:
        'bg-destructive text-destructive-foreground shadow-xs hover:bg-destructive-hover',
      outline:
        'bg-background shadow-xs hover:bg-accent hover:text-accent-foreground dark:bg-input/30 dark:border-input dark:hover:bg-input/50 border',
      secondary: 'bg-secondary text-secondary-foreground shadow-xs hover:bg-secondary/80',
      ghost: 'hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50',
      link: 'text-primary underline-offset-4 hover:underline'
    },
    size: {
      default: 'h-9 px-4 py-2 has-[>svg]:px-3',
      sm: 'h-8 gap-1.5 rounded-md px-3 has-[>svg]:px-2.5',
      lg: 'h-10 rounded-md px-6 has-[>svg]:px-4',
      icon: 'size-9'
    }
  },
  defaultVariants: {
    variant: 'default',
    size: 'default'
  }
});

export type ButtonVariant = VariantProps<typeof buttonVariants>['variant'];
export type ButtonSize = VariantProps<typeof buttonVariants>['size'];

export type ButtonProps = WithElementRef<HTMLButtonAttributes> &
  WithElementRef<HTMLAnchorAttributes> & {
    variant?: ButtonVariant;
    size?: ButtonSize;
  };
