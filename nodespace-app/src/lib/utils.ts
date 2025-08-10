import { type ClassValue, clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

// Type utilities for shadcn-svelte components
export type WithElementRef<T> = T & {
  ref?: HTMLElement | null;
};

export type WithoutChildren<T> = Omit<T, 'children'>;
