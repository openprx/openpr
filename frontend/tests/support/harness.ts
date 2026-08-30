/**
 * Minimal check harness shared by the `test:flow-v0.4` suites.
 *
 * Deliberately not a test framework: these suites run as plain `bun tests/<file>.ts` programs
 * (the convention `tests/record-values-roundtrip.test.ts` and `tests/flow-limits-negotiation.
 * test.ts` already use), and the v0.4 gate needs a machine-readable per-suite verdict it can put
 * in evidence, not a pretty reporter.
 *
 * `skip()` exists and is counted SEPARATELY from `pass`, and any run with a non-zero skip count
 * reports `passed: false` at the runner level unless the runner was told the skip is expected.
 * A suite that quietly reports "0 failures" while having executed nothing is the exact failure
 * mode these gates are supposed to make impossible.
 */

export interface SuiteResult {
	readonly name: string;
	readonly passed: number;
	readonly failed: number;
	readonly skipped: number;
	readonly durationMs: number;
	readonly failures: readonly string[];
	readonly skips: readonly string[];
}

export class Suite {
	readonly name: string;
	private passedCount = 0;
	private readonly failureMessages: string[] = [];
	private readonly skipMessages: string[] = [];
	private readonly startedAt = Date.now();

	constructor(name: string) {
		this.name = name;
	}

	check(label: string, body: () => void): void {
		try {
			body();
			this.passedCount += 1;
			console.log(`  ok   ${label}`);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			this.failureMessages.push(`${label}: ${message}`);
			console.log(`  FAIL ${label}\n       ${message.split('\n').join('\n       ')}`);
		}
	}

	async checkAsync(label: string, body: () => Promise<void>): Promise<void> {
		try {
			await body();
			this.passedCount += 1;
			console.log(`  ok   ${label}`);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			this.failureMessages.push(`${label}: ${message}`);
			console.log(`  FAIL ${label}\n       ${message.split('\n').join('\n       ')}`);
		}
	}

	/** Records a check that did NOT run, with the reason. Never counted as a pass. */
	skip(label: string, reason: string): void {
		this.skipMessages.push(`${label}: ${reason}`);
		console.log(`  SKIP ${label} -- ${reason}`);
	}

	result(): SuiteResult {
		return {
			name: this.name,
			passed: this.passedCount,
			failed: this.failureMessages.length,
			skipped: this.skipMessages.length,
			durationMs: Date.now() - this.startedAt,
			failures: [...this.failureMessages],
			skips: [...this.skipMessages]
		};
	}
}

/** Prints a suite's verdict and exits non-zero on any failure, for standalone `bun tests/x.ts`
 * runs. The aggregate runner imports the suites instead and does its own exit. */
export function finish(result: SuiteResult): never {
	console.log(
		`\n${result.name}: ${result.passed} passed, ${result.failed} failed, ${result.skipped} skipped (${result.durationMs} ms)`
	);
	process.exit(result.failed > 0 ? 1 : 0);
}

export function assert(condition: unknown, message: string): asserts condition {
	if (!condition) throw new Error(message);
}

export function assertEqual<T>(actual: T, expected: T, message: string): void {
	if (!Object.is(actual, expected)) {
		throw new Error(`${message}\n  expected: ${format(expected)}\n  actual:   ${format(actual)}`);
	}
}

export function assertDeepEqual(actual: unknown, expected: unknown, message: string): void {
	const a = JSON.stringify(actual);
	const b = JSON.stringify(expected);
	if (a !== b) {
		throw new Error(`${message}\n  expected: ${b}\n  actual:   ${a}`);
	}
}

export function assertNotEqual<T>(actual: T, unexpected: T, message: string): void {
	if (Object.is(actual, unexpected)) {
		throw new Error(`${message}\n  both sides were: ${format(actual)}`);
	}
}

function format(value: unknown): string {
	if (typeof value === 'string') return JSON.stringify(value);
	try {
		return JSON.stringify(value);
	} catch {
		return String(value);
	}
}

/** Waits until `predicate()` is true or the budget expires. Used only where a real event loop
 * turn is genuinely required (a mock socket's queued callbacks); never as a sleep-and-hope. */
export async function waitFor(
	predicate: () => boolean,
	message: string,
	budgetMs = 2000
): Promise<void> {
	const deadline = Date.now() + budgetMs;
	while (Date.now() < deadline) {
		if (predicate()) return;
		await new Promise((resolve) => setTimeout(resolve, 5));
	}
	throw new Error(`timed out after ${budgetMs} ms waiting for: ${message}`);
}
