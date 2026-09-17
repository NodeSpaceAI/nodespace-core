/**
 * Badge component types
 *
 * The filled variants (`default`, `destructive`) hover to their own opaque
 * `--*-hover` token rather than an alpha fill, for the same reasons the sibling
 * Button does — see `button/types.ts` and DESIGN.md's Button section. An alpha
 * hover composites against whatever is behind it, so it lightens on a white page
 * and darkens on a dark one, flipping direction between themes as a side effect
 * rather than a decision.
 *
 * The two stock shadcn treatments on `destructive` were dropped with it, because
 * they only made sense together with that alpha hover: `dark:bg-destructive/70`
 * made the dark rest fill translucent, which was the one thing propping up the
 * `text-white` below it (white is 2.86:1 on the solid `--destructive` fill, far
 * under AA). `--destructive-foreground` carries real per-theme values precisely
 * because white fails on dark-mode fills.
 *
 * Every hover here is gated behind `[a&]:` and so only applies when the Badge
 * renders as an anchor, which no consumer does today. That makes the defect
 * latent rather than visible — it would be inherited by the first linked filled
 * badge — not absent, which is why it is fixed rather than left.
 *
 * `secondary` keeps its alpha hover: it is a neutral surface at ~16:1 that GAINS
 * contrast on hover, not a semantic fill. `filled-variants.test.ts` holds
 * both this and `buttonVariants` to the rule.
 */
import { type VariantProps, tv } from 'tailwind-variants';

export const badgeVariants = tv({
  base: 'aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive inline-flex w-fit shrink-0 items-center justify-center gap-1 overflow-hidden whitespace-nowrap rounded-md border px-2 py-0.5 text-xs font-medium transition-[color,box-shadow] [&>svg]:pointer-events-none [&>svg]:size-3',
  variants: {
    variant: {
      default:
        'bg-primary text-primary-foreground [a&]:hover:bg-primary-hover border-transparent',
      secondary:
        'bg-secondary text-secondary-foreground [a&]:hover:bg-secondary/90 border-transparent',
      destructive:
        'bg-destructive text-destructive-foreground [a&]:hover:bg-destructive-hover border-transparent',
      outline: 'text-foreground [a&]:hover:bg-accent [a&]:hover:text-accent-foreground'
    }
  },
  defaultVariants: {
    variant: 'default'
  }
});

export type BadgeVariant = VariantProps<typeof badgeVariants>['variant'];
