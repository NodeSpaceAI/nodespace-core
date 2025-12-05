import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { throttle } from '$lib/utils/throttle';

describe('throttle', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  describe('basic throttling behavior', () => {
    it('should call the function immediately on first invocation', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 100);

      throttled();

      expect(mockFn).toHaveBeenCalledTimes(1);
    });

    it('should not call the function again during throttle period', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 100);

      throttled();
      throttled();
      throttled();

      expect(mockFn).toHaveBeenCalledTimes(1);
    });

    it('should allow the function to be called again after throttle period expires', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 100);

      throttled();
      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(100);

      throttled();
      expect(mockFn).toHaveBeenCalledTimes(2);
    });

    it('should ignore multiple calls during throttle period', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 100);

      throttled();
      throttled();
      throttled();

      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(50);
      throttled();
      throttled();

      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(50);
      throttled();

      expect(mockFn).toHaveBeenCalledTimes(2);
    });
  });

  describe('function arguments', () => {
    it('should pass arguments to the throttled function', () => {
      const mockFn = vi.fn((a: number, b: string) => a + b);
      const throttled = throttle(mockFn, 100);

      throttled(42, 'test');

      expect(mockFn).toHaveBeenCalledWith(42, 'test');
    });

    it('should pass arguments correctly on subsequent calls', () => {
      const mockFn = vi.fn((a: number) => a * 2);
      const throttled = throttle(mockFn, 100);

      throttled(1);
      expect(mockFn).toHaveBeenCalledWith(1);

      vi.advanceTimersByTime(100);

      throttled(2);
      expect(mockFn).toHaveBeenCalledWith(2);
    });

    it('should handle no arguments', () => {
      const mockFn = vi.fn(() => 'called');
      const throttled = throttle(mockFn, 100);

      throttled();

      expect(mockFn).toHaveBeenCalledWith();
    });

    it('should handle multiple arguments of different types', () => {
      const mockFn = vi.fn((_a: number, _b: string, _c: boolean, _d: object) => {});
      const throttled = throttle(mockFn, 100);

      const obj = { key: 'value' };
      throttled(42, 'test', true, obj);

      expect(mockFn).toHaveBeenCalledWith(42, 'test', true, obj);
    });
  });

  describe('return values', () => {
    it('should return the result of the function on first call', () => {
      const mockFn = vi.fn(() => 'result');
      const throttled = throttle(mockFn, 100);

      const result = throttled();

      expect(result).toBe('result');
    });

    it('should return undefined during throttle period', () => {
      const mockFn = vi.fn(() => 'result');
      const throttled = throttle(mockFn, 100);

      throttled();
      const result = throttled();

      expect(result).toBeUndefined();
    });

    it('should return the new result after throttle period expires', () => {
      const mockFn = vi.fn((x: number) => x * 2);
      const throttled = throttle(mockFn, 100);

      const result1 = throttled(5);
      expect(result1).toBe(10);

      vi.advanceTimersByTime(100);

      const result2 = throttled(7);
      expect(result2).toBe(14);
    });

    it('should handle functions returning complex types', () => {
      const mockFn = vi.fn(() => ({ data: [1, 2, 3], status: 'ok' }));
      const throttled = throttle(mockFn, 100);

      const result = throttled();

      expect(result).toEqual({ data: [1, 2, 3], status: 'ok' });
    });
  });

  describe('this context binding', () => {
    it('should preserve this context when called as method', () => {
      const obj = {
        value: 42,
        getValue: function (this: { value: number }) {
          return this.value;
        }
      };

      obj.getValue = throttle(obj.getValue, 100);

      const result = obj.getValue();
      expect(result).toBe(42);
    });

    it('should preserve this context across multiple calls', () => {
      const obj = {
        counter: 0,
        increment: function (this: { counter: number }) {
          this.counter++;
          return this.counter;
        }
      };

      obj.increment = throttle(obj.increment, 100);

      expect(obj.increment()).toBe(1);

      vi.advanceTimersByTime(100);

      expect(obj.increment()).toBe(2);
      expect(obj.counter).toBe(2);
    });

    it('should handle arrow functions without this context', () => {
      const mockFn = vi.fn(() => 'no context');
      const throttled = throttle(mockFn, 100);

      const result = throttled();
      expect(result).toBe('no context');
    });
  });

  describe('timing precision', () => {
    it('should respect exact timing boundaries', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 100);

      throttled();
      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(99);
      throttled();
      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(1);
      throttled();
      expect(mockFn).toHaveBeenCalledTimes(2);
    });

    it('should handle different throttle limits', () => {
      const mockFn1 = vi.fn();
      const throttled1 = throttle(mockFn1, 50);

      const mockFn2 = vi.fn();
      const throttled2 = throttle(mockFn2, 200);

      throttled1();
      throttled2();

      expect(mockFn1).toHaveBeenCalledTimes(1);
      expect(mockFn2).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(50);

      throttled1();
      throttled2();

      expect(mockFn1).toHaveBeenCalledTimes(2);
      expect(mockFn2).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(150);

      throttled1();
      throttled2();

      expect(mockFn1).toHaveBeenCalledTimes(3);
      expect(mockFn2).toHaveBeenCalledTimes(2);
    });

    it('should handle zero-millisecond throttle (edge case)', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 0);

      throttled();
      expect(mockFn).toHaveBeenCalledTimes(1);

      throttled();
      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(0);

      throttled();
      expect(mockFn).toHaveBeenCalledTimes(2);
    });

    it('should handle very long throttle periods', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 10000);

      throttled();
      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(9999);
      throttled();
      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(1);
      throttled();
      expect(mockFn).toHaveBeenCalledTimes(2);
    });
  });

  describe('multiple throttled instances', () => {
    it('should maintain independent throttle states', () => {
      const mockFn = vi.fn();
      const throttled1 = throttle(mockFn, 100);
      const throttled2 = throttle(mockFn, 100);

      throttled1();
      expect(mockFn).toHaveBeenCalledTimes(1);

      throttled2();
      expect(mockFn).toHaveBeenCalledTimes(2);

      throttled1();
      throttled2();
      expect(mockFn).toHaveBeenCalledTimes(2);
    });

    it('should handle different functions independently', () => {
      const mockFn1 = vi.fn();
      const mockFn2 = vi.fn();
      const throttled1 = throttle(mockFn1, 100);
      const throttled2 = throttle(mockFn2, 100);

      throttled1();
      throttled2();

      expect(mockFn1).toHaveBeenCalledTimes(1);
      expect(mockFn2).toHaveBeenCalledTimes(1);

      throttled1();
      throttled2();

      expect(mockFn1).toHaveBeenCalledTimes(1);
      expect(mockFn2).toHaveBeenCalledTimes(1);
    });
  });

  describe('rapid successive calls', () => {
    it('should handle rapid succession of calls correctly', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 100);

      for (let i = 0; i < 10; i++) {
        throttled();
      }

      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(100);

      for (let i = 0; i < 10; i++) {
        throttled();
      }

      expect(mockFn).toHaveBeenCalledTimes(2);
    });

    it('should handle calls at regular intervals', () => {
      const mockFn = vi.fn();
      const throttled = throttle(mockFn, 100);

      throttled();
      expect(mockFn).toHaveBeenCalledTimes(1);

      for (let i = 0; i < 5; i++) {
        vi.advanceTimersByTime(100);
        throttled();
      }

      expect(mockFn).toHaveBeenCalledTimes(6);
    });
  });

  describe('error handling', () => {
    it('should propagate errors from the throttled function', () => {
      const error = new Error('test error');
      const mockFn = vi.fn(() => {
        throw error;
      });
      const throttled = throttle(mockFn, 100);

      expect(() => throttled()).toThrow('test error');
    });

    it('should reset throttle state after error', () => {
      let shouldThrow = true;
      const mockFn = vi.fn(() => {
        if (shouldThrow) {
          throw new Error('test error');
        }
        return 'success';
      });
      const throttled = throttle(mockFn, 100);

      expect(() => throttled()).toThrow('test error');
      expect(mockFn).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(100);
      shouldThrow = false;

      const result = throttled();
      expect(result).toBe('success');
      expect(mockFn).toHaveBeenCalledTimes(2);
    });
  });

  describe('real-world scenarios', () => {
    it('should work like a window resize handler', () => {
      const handleResize = vi.fn((width: number) => {
        return `Resized to ${width}px`;
      });
      const throttledResize = throttle(handleResize, 100);

      // Simulate rapid resize events
      throttledResize(800);
      throttledResize(850);
      throttledResize(900);
      throttledResize(950);
      throttledResize(1000);

      expect(handleResize).toHaveBeenCalledTimes(1);
      expect(handleResize).toHaveBeenCalledWith(800);

      vi.advanceTimersByTime(100);

      throttledResize(1024);

      expect(handleResize).toHaveBeenCalledTimes(2);
      expect(handleResize).toHaveBeenLastCalledWith(1024);
    });

    it('should work like a scroll handler', () => {
      const handleScroll = vi.fn((scrollY: number) => {
        return scrollY > 100 ? 'show-header' : 'hide-header';
      });
      const throttledScroll = throttle(handleScroll, 100);

      // Simulate rapid scroll events
      for (let i = 0; i < 20; i++) {
        throttledScroll(i * 10);
      }

      expect(handleScroll).toHaveBeenCalledTimes(1);

      vi.advanceTimersByTime(100);

      throttledScroll(500);

      expect(handleScroll).toHaveBeenCalledTimes(2);
    });

    it('should work like an input handler', () => {
      const handleInput = vi.fn((value: string) => {
        return value.length;
      });
      const throttledInput = throttle(handleInput, 50);

      throttledInput('a');
      throttledInput('ab');
      throttledInput('abc');

      expect(handleInput).toHaveBeenCalledTimes(1);
      expect(handleInput).toHaveBeenCalledWith('a');

      vi.advanceTimersByTime(50);

      throttledInput('abcd');

      expect(handleInput).toHaveBeenCalledTimes(2);
      expect(handleInput).toHaveBeenLastCalledWith('abcd');
    });

    it('should work with API rate limiting scenario', () => {
      const apiCall = vi.fn((query: string) => {
        return { results: [query] };
      });
      const throttledApiCall = throttle(apiCall, 1000);

      // User types rapidly
      throttledApiCall('h');
      throttledApiCall('he');
      throttledApiCall('hel');
      throttledApiCall('hell');
      throttledApiCall('hello');

      // Only first call goes through
      expect(apiCall).toHaveBeenCalledTimes(1);
      expect(apiCall).toHaveBeenCalledWith('h');

      vi.advanceTimersByTime(1000);

      throttledApiCall('hello world');

      expect(apiCall).toHaveBeenCalledTimes(2);
      expect(apiCall).toHaveBeenLastCalledWith('hello world');
    });
  });

  describe('type safety and generics', () => {
    it('should preserve function signature with number return', () => {
      const add = (a: number, b: number): number => a + b;
      const throttledAdd = throttle(add, 100);

      const result = throttledAdd(1, 2);
      expect(result).toBe(3);
    });

    it('should preserve function signature with string return', () => {
      const concat = (a: string, b: string): string => a + b;
      const throttledConcat = throttle(concat, 100);

      const result = throttledConcat('hello', ' world');
      expect(result).toBe('hello world');
    });

    it('should preserve function signature with void return', () => {
      const voidFn = vi.fn((_arg: string): void => {});
      const throttledVoidFn = throttle(voidFn, 100);

      const result = throttledVoidFn('test');
      expect(result).toBeUndefined();
      expect(voidFn).toHaveBeenCalledWith('test');
    });
  });
});
