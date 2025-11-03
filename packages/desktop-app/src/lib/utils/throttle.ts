/**
 * Throttle function - Limits how often a function can be called
 *
 * Ensures a function is called at most once per specified time period.
 * Useful for performance optimization of frequent events like resize, scroll, or mousemove.
 *
 * @param func - The function to throttle
 * @param limit - Minimum time (in milliseconds) between function calls
 * @returns Throttled version of the function
 *
 * @example
 * const throttledResize = throttle((width: number) => {
 *   console.log('Window resized to:', width);
 * }, 100);
 *
 * window.addEventListener('resize', () => throttledResize(window.innerWidth));
 */
export function throttle<T extends (...args: never[]) => unknown>(func: T, limit: number): T {
  let inThrottle: boolean;

  return function (this: ThisParameterType<T>, ...args: Parameters<T>): ReturnType<T> | undefined {
    if (!inThrottle) {
      const result = func.apply(this, args) as ReturnType<T>;
      inThrottle = true;

      setTimeout(() => {
        inThrottle = false;
      }, limit);

      return result;
    }
    return undefined;
  } as T;
}
