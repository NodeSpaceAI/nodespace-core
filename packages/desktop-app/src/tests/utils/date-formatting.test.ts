/**
 * Unit tests for date-formatting utility
 *
 * Tests all date formatting functions used across the application:
 * - normalizeDate: Strip time components from dates
 * - formatDateTitle: Human-readable date strings (Today, Tomorrow, Yesterday, YYYY-MM-DD)
 * - formatDateISO: ISO date format (YYYY-MM-DD)
 * - parseDateString: Parse YYYY-MM-DD strings into Date objects
 * - isValidDateString: Validate YYYY-MM-DD format strings
 */
import { describe, it, expect } from 'vitest';
import {
	normalizeDate,
	formatDateTitle,
	formatDateISO,
	parseDateString,
	isValidDateString
} from '$lib/utils/date-formatting';

describe('date-formatting utils', () => {
	describe('normalizeDate', () => {
		it('should strip time components and return midnight local time', () => {
			const date = new Date(2025, 0, 15, 14, 30, 45, 123); // Jan 15, 2025 14:30:45.123
			const normalized = normalizeDate(date);

			expect(normalized.getFullYear()).toBe(2025);
			expect(normalized.getMonth()).toBe(0);
			expect(normalized.getDate()).toBe(15);
			expect(normalized.getHours()).toBe(0);
			expect(normalized.getMinutes()).toBe(0);
			expect(normalized.getSeconds()).toBe(0);
			expect(normalized.getMilliseconds()).toBe(0);
		});

		it('should handle dates already at midnight', () => {
			const date = new Date(2025, 0, 15, 0, 0, 0, 0);
			const normalized = normalizeDate(date);

			expect(normalized.getTime()).toBe(date.getTime());
		});

		it('should handle edge of day (23:59:59)', () => {
			const date = new Date(2025, 0, 15, 23, 59, 59, 999);
			const normalized = normalizeDate(date);

			expect(normalized.getFullYear()).toBe(2025);
			expect(normalized.getMonth()).toBe(0);
			expect(normalized.getDate()).toBe(15);
			expect(normalized.getHours()).toBe(0);
		});
	});

	describe('formatDateTitle', () => {
		describe('relative dates', () => {
			it('should return "Today" for current date', () => {
				const today = new Date();
				const result = formatDateTitle(today);
				expect(result).toBe('Today');
			});

			it('should return "Today" for current date with different time', () => {
				const today = new Date();
				today.setHours(23, 59, 59, 999);
				const result = formatDateTitle(today);
				expect(result).toBe('Today');
			});

			it('should return "Tomorrow" for next day', () => {
				const tomorrow = new Date();
				tomorrow.setDate(tomorrow.getDate() + 1);
				const result = formatDateTitle(tomorrow);
				expect(result).toBe('Tomorrow');
			});

			it('should return "Yesterday" for previous day', () => {
				const yesterday = new Date();
				yesterday.setDate(yesterday.getDate() - 1);
				const result = formatDateTitle(yesterday);
				expect(result).toBe('Yesterday');
			});
		});

		describe('absolute dates', () => {
			it('should return YYYY-MM-DD for dates beyond yesterday/tomorrow', () => {
				const future = new Date(2025, 11, 25); // Dec 25, 2025
				const result = formatDateTitle(future);
				expect(result).toBe('2025-12-25');
			});

			it('should return YYYY-MM-DD for dates in the past', () => {
				const past = new Date(2020, 0, 1); // Jan 1, 2020
				const result = formatDateTitle(past);
				expect(result).toBe('2020-01-01');
			});

			it('should pad single-digit months and days', () => {
				const date = new Date(2025, 0, 5); // Jan 5, 2025
				const result = formatDateTitle(date);
				expect(result).toBe('2025-01-05');
			});

			it('should handle leap year dates', () => {
				const leapDay = new Date(2024, 1, 29); // Feb 29, 2024
				const result = formatDateTitle(leapDay);
				expect(result).toBe('2024-02-29');
			});
		});

		describe('edge cases', () => {
			it('should handle year boundaries', () => {
				const newYear = new Date(2025, 0, 1); // Jan 1, 2025
				const result = formatDateTitle(newYear);
				expect(result).toMatch(/^(\d{4}-\d{2}-\d{2}|Today|Tomorrow|Yesterday)$/);
			});

			it('should handle month boundaries', () => {
				const monthEnd = new Date(2025, 0, 31); // Jan 31, 2025
				const result = formatDateTitle(monthEnd);
				expect(result).toMatch(/^(\d{4}-\d{2}-\d{2}|Today|Tomorrow|Yesterday)$/);
			});

			it('should normalize time components before comparison', () => {
				const today = new Date();
				const todayAtNoon = new Date(
					today.getFullYear(),
					today.getMonth(),
					today.getDate(),
					12,
					0,
					0
				);
				const result = formatDateTitle(todayAtNoon);
				expect(result).toBe('Today');
			});
		});
	});

	describe('formatDateISO', () => {
		it('should format date as YYYY-MM-DD', () => {
			const date = new Date(2025, 0, 15); // Jan 15, 2025
			const result = formatDateISO(date);
			expect(result).toBe('2025-01-15');
		});

		it('should pad single-digit months', () => {
			const date = new Date(2025, 0, 15); // January = month 0
			const result = formatDateISO(date);
			expect(result).toBe('2025-01-15');
		});

		it('should pad single-digit days', () => {
			const date = new Date(2025, 0, 5); // Day 5
			const result = formatDateISO(date);
			expect(result).toBe('2025-01-05');
		});

		it('should handle double-digit months and days', () => {
			const date = new Date(2025, 11, 25); // Dec 25
			const result = formatDateISO(date);
			expect(result).toBe('2025-12-25');
		});

		it('should ignore time components', () => {
			const date = new Date(2025, 0, 15, 23, 59, 59, 999);
			const result = formatDateISO(date);
			expect(result).toBe('2025-01-15');
		});

		it('should handle leap year dates', () => {
			const leapDay = new Date(2024, 1, 29); // Feb 29, 2024
			const result = formatDateISO(leapDay);
			expect(result).toBe('2024-02-29');
		});

		it('should handle year boundaries', () => {
			const newYear = new Date(2025, 0, 1);
			const result = formatDateISO(newYear);
			expect(result).toBe('2025-01-01');
		});
	});

	describe('parseDateString', () => {
		describe('valid dates', () => {
			it('should parse valid YYYY-MM-DD string', () => {
				const result = parseDateString('2025-01-15');
				expect(result).not.toBeNull();
				expect(result?.getFullYear()).toBe(2025);
				expect(result?.getMonth()).toBe(0); // January = 0
				expect(result?.getDate()).toBe(15);
			});

			it('should parse date with single-digit month and day', () => {
				const result = parseDateString('2025-01-05');
				expect(result).not.toBeNull();
				expect(result?.getMonth()).toBe(0);
				expect(result?.getDate()).toBe(5);
			});

			it('should parse date with double-digit month and day', () => {
				const result = parseDateString('2025-12-25');
				expect(result).not.toBeNull();
				expect(result?.getMonth()).toBe(11); // December = 11
				expect(result?.getDate()).toBe(25);
			});

			it('should normalize parsed dates to midnight', () => {
				const result = parseDateString('2025-01-15');
				expect(result?.getHours()).toBe(0);
				expect(result?.getMinutes()).toBe(0);
				expect(result?.getSeconds()).toBe(0);
				expect(result?.getMilliseconds()).toBe(0);
			});

			it('should parse leap year date', () => {
				const result = parseDateString('2024-02-29');
				expect(result).not.toBeNull();
				expect(result?.getFullYear()).toBe(2024);
				expect(result?.getMonth()).toBe(1);
				expect(result?.getDate()).toBe(29);
			});
		});

		describe('invalid dates - syntax errors', () => {
			it('should return null for empty string', () => {
				const result = parseDateString('');
				expect(result).toBeNull();
			});

			it('should return null for malformed string', () => {
				const result = parseDateString('not-a-date');
				expect(result).toBeNull();
			});

			it('should return null for incomplete date (missing day)', () => {
				const result = parseDateString('2025-01');
				expect(result).toBeNull();
			});

			it('should return null for incomplete date (missing month and day)', () => {
				const result = parseDateString('2025');
				expect(result).toBeNull();
			});

			it('should return null for non-numeric components', () => {
				const result = parseDateString('2025-AB-CD');
				expect(result).toBeNull();
			});
		});

		describe('invalid dates - semantic errors', () => {
			it('should return null for invalid month (month 13)', () => {
				const result = parseDateString('2025-13-01');
				expect(result).toBeNull();
			});

			it('should return null for invalid day (Feb 30)', () => {
				const result = parseDateString('2025-02-30');
				expect(result).toBeNull();
			});

			it('should return null for invalid day (Feb 31)', () => {
				const result = parseDateString('2025-02-31');
				expect(result).toBeNull();
			});

			it('should return null for invalid day (day 32)', () => {
				const result = parseDateString('2025-01-32');
				expect(result).toBeNull();
			});

			it('should return null for Feb 29 in non-leap year', () => {
				const result = parseDateString('2025-02-29');
				expect(result).toBeNull();
			});

			it('should return null for month 0', () => {
				const result = parseDateString('2025-00-15');
				expect(result).toBeNull();
			});

			it('should return null for day 0', () => {
				const result = parseDateString('2025-01-00');
				expect(result).toBeNull();
			});

			it('should return null for 31-day month with day 31 in 30-day month (April)', () => {
				const result = parseDateString('2025-04-31');
				expect(result).toBeNull();
			});
		});

		describe('edge cases', () => {
			it('should handle year boundaries', () => {
				const result = parseDateString('2025-01-01');
				expect(result).not.toBeNull();
				expect(result?.getFullYear()).toBe(2025);
			});

			it('should handle month boundaries', () => {
				const result = parseDateString('2025-01-31');
				expect(result).not.toBeNull();
				expect(result?.getDate()).toBe(31);
			});

			it('should handle very far future dates', () => {
				const result = parseDateString('9999-12-31');
				expect(result).not.toBeNull();
				expect(result?.getFullYear()).toBe(9999);
			});

			it('should handle very far past dates', () => {
				const result = parseDateString('1000-01-01');
				expect(result).not.toBeNull();
				expect(result?.getFullYear()).toBe(1000);
			});
		});
	});

	describe('isValidDateString', () => {
		describe('valid date strings', () => {
			it('should return true for valid YYYY-MM-DD format', () => {
				expect(isValidDateString('2025-01-15')).toBe(true);
			});

			it('should return true for leap year date', () => {
				expect(isValidDateString('2024-02-29')).toBe(true);
			});

			it('should return true for edge of year', () => {
				expect(isValidDateString('2025-12-31')).toBe(true);
			});

			it('should return true for start of year', () => {
				expect(isValidDateString('2025-01-01')).toBe(true);
			});
		});

		describe('invalid syntax', () => {
			it('should return false for empty string', () => {
				expect(isValidDateString('')).toBe(false);
			});

			it('should return false for wrong format (MM-DD-YYYY)', () => {
				expect(isValidDateString('01-15-2025')).toBe(false);
			});

			it('should return false for wrong format (DD-MM-YYYY)', () => {
				expect(isValidDateString('15-01-2025')).toBe(false);
			});

			it('should return false for slash separators', () => {
				expect(isValidDateString('2025/01/15')).toBe(false);
			});

			it('should return false for no separators', () => {
				expect(isValidDateString('20250115')).toBe(false);
			});

			it('should return false for incomplete date', () => {
				expect(isValidDateString('2025-01')).toBe(false);
			});

			it('should return false for extra components', () => {
				expect(isValidDateString('2025-01-15-00')).toBe(false);
			});

			it('should return false for single-digit year', () => {
				expect(isValidDateString('5-01-15')).toBe(false);
			});

			it('should return false for two-digit year', () => {
				expect(isValidDateString('25-01-15')).toBe(false);
			});

			it('should return false for three-digit year', () => {
				expect(isValidDateString('025-01-15')).toBe(false);
			});

			it('should return false for single-digit month without padding', () => {
				expect(isValidDateString('2025-1-15')).toBe(false);
			});

			it('should return false for single-digit day without padding', () => {
				expect(isValidDateString('2025-01-5')).toBe(false);
			});

			it('should return false for three-digit month', () => {
				expect(isValidDateString('2025-001-15')).toBe(false);
			});

			it('should return false for three-digit day', () => {
				expect(isValidDateString('2025-01-015')).toBe(false);
			});
		});

		describe('invalid semantics', () => {
			it('should return false for Feb 30', () => {
				expect(isValidDateString('2025-02-30')).toBe(false);
			});

			it('should return false for Feb 29 in non-leap year', () => {
				expect(isValidDateString('2025-02-29')).toBe(false);
			});

			it('should return false for month 13', () => {
				expect(isValidDateString('2025-13-01')).toBe(false);
			});

			it('should return false for month 00', () => {
				expect(isValidDateString('2025-00-15')).toBe(false);
			});

			it('should return false for day 00', () => {
				expect(isValidDateString('2025-01-00')).toBe(false);
			});

			it('should return false for day 32', () => {
				expect(isValidDateString('2025-01-32')).toBe(false);
			});

			it('should return false for April 31 (30-day month)', () => {
				expect(isValidDateString('2025-04-31')).toBe(false);
			});

			it('should return false for June 31 (30-day month)', () => {
				expect(isValidDateString('2025-06-31')).toBe(false);
			});

			it('should return false for September 31 (30-day month)', () => {
				expect(isValidDateString('2025-09-31')).toBe(false);
			});

			it('should return false for November 31 (30-day month)', () => {
				expect(isValidDateString('2025-11-31')).toBe(false);
			});
		});

		describe('edge cases', () => {
			it('should handle whitespace padding', () => {
				expect(isValidDateString(' 2025-01-15 ')).toBe(false);
			});

			it('should handle very far future dates', () => {
				expect(isValidDateString('9999-12-31')).toBe(true);
			});

			it('should handle very far past dates', () => {
				expect(isValidDateString('1000-01-01')).toBe(true);
			});

			it('should handle special characters', () => {
				expect(isValidDateString('2025-01-15T00:00:00')).toBe(false);
			});

			it('should handle null/undefined-like strings', () => {
				expect(isValidDateString('null')).toBe(false);
				expect(isValidDateString('undefined')).toBe(false);
			});
		});
	});

	describe('integration - round-trip conversions', () => {
		it('should round-trip: Date -> formatDateISO -> parseDateString -> Date', () => {
			const original = new Date(2025, 0, 15);
			const isoString = formatDateISO(original);
			const parsed = parseDateString(isoString);

			expect(parsed).not.toBeNull();
			expect(parsed?.getFullYear()).toBe(original.getFullYear());
			expect(parsed?.getMonth()).toBe(original.getMonth());
			expect(parsed?.getDate()).toBe(original.getDate());
		});

		it('should validate parsed dates as valid strings', () => {
			const dateString = '2025-01-15';
			const parsed = parseDateString(dateString);
			expect(parsed).not.toBeNull();
			expect(isValidDateString(dateString)).toBe(true);
		});

		it('should reject invalid strings in both parseDateString and isValidDateString', () => {
			const invalidDate = '2025-02-30';
			expect(parseDateString(invalidDate)).toBeNull();
			expect(isValidDateString(invalidDate)).toBe(false);
		});

		it('should normalize dates consistently across functions', () => {
			const date = new Date(2025, 0, 15, 14, 30, 45);
			const normalized = normalizeDate(date);
			const formatted = formatDateISO(normalized);
			const parsed = parseDateString(formatted);

			expect(parsed?.getTime()).toBe(normalized.getTime());
		});
	});

	describe('real-world scenarios', () => {
		it('should handle date tab navigation flow', () => {
			const today = new Date();
			const title = formatDateTitle(today);
			expect(title).toBe('Today');

			const isoString = formatDateISO(today);
			expect(isValidDateString(isoString)).toBe(true);

			const parsed = parseDateString(isoString);
			expect(parsed).not.toBeNull();
		});

		it('should handle date input validation flow', () => {
			const userInput = '2025-12-25';

			// First validate
			expect(isValidDateString(userInput)).toBe(true);

			// Then parse
			const date = parseDateString(userInput);
			expect(date).not.toBeNull();

			// Then display
			const title = formatDateTitle(date!);
			expect(title).toBe('2025-12-25');
		});

		it('should reject user input with invalid dates', () => {
			const invalidInputs = [
				'2025-13-01', // Invalid month
				'2025-02-30', // Invalid day
				'2025/12/25', // Wrong separator
				'12-25-2025', // Wrong format
				'not a date', // Garbage input
				'' // Empty input
			];

			invalidInputs.forEach((input) => {
				expect(isValidDateString(input)).toBe(false);
				expect(parseDateString(input)).toBeNull();
			});
		});

		it('should handle all months with correct day limits', () => {
			// 31-day months
			expect(isValidDateString('2025-01-31')).toBe(true);
			expect(isValidDateString('2025-03-31')).toBe(true);
			expect(isValidDateString('2025-05-31')).toBe(true);
			expect(isValidDateString('2025-07-31')).toBe(true);
			expect(isValidDateString('2025-08-31')).toBe(true);
			expect(isValidDateString('2025-10-31')).toBe(true);
			expect(isValidDateString('2025-12-31')).toBe(true);

			// 30-day months should reject day 31
			expect(isValidDateString('2025-04-31')).toBe(false);
			expect(isValidDateString('2025-06-31')).toBe(false);
			expect(isValidDateString('2025-09-31')).toBe(false);
			expect(isValidDateString('2025-11-31')).toBe(false);

			// 30-day months should accept day 30
			expect(isValidDateString('2025-04-30')).toBe(true);
			expect(isValidDateString('2025-06-30')).toBe(true);
			expect(isValidDateString('2025-09-30')).toBe(true);
			expect(isValidDateString('2025-11-30')).toBe(true);
		});
	});
});
