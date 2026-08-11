/**
 * Round-trip coverage for the record draft <-> record value mapping.
 *
 * Run with: `bun run test:record-values`
 *
 * The API replaces the whole `values` map on update, so a field that survives
 * `value -> draft -> value` unchanged is a field that cannot be silently wiped
 * by an edit. Every `FormFieldType` must appear in `CASES`; the guard at the
 * bottom fails when a new type is added without a case.
 */

import assert from 'node:assert/strict';
import type { FormField, FormFieldType } from '../src/lib/api/forms';
import {
	FORM_FIELD_TYPES,
	draftValueFromRecordValue,
	isMissingRequiredValue,
	recordValueFromDraftValue,
	type DraftToValueContext
} from '../src/lib/forms/record-values';

interface RoundTripCase {
	field: FormField;
	/** Value as the API stores it. */
	stored: unknown;
	/** Draft the editor controls must receive. */
	draft: unknown;
	/** Value the payload must carry back. Defaults to `stored`. */
	submitted?: unknown;
	context?: DraftToValueContext;
}

const context: DraftToValueContext = {
	relationTarget: (_field, recordId) =>
		recordId === 'rec-1' ? { display: 'Order #1', form_key: 'orders' } : undefined,
	member: (userId) => (userId === 'user-1' ? { name: 'Ada', email: 'ada@example.test' } : undefined)
};

function field(key: string, type: FormFieldType, extra: Partial<FormField> = {}): FormField {
	return { key, type, ...extra };
}

const CASES: Record<FormFieldType, RoundTripCase> = {
	text: { field: field('note', 'text'), stored: 'hello', draft: 'hello' },
	phone: { field: field('phone', 'phone'), stored: '+1 555 0100', draft: '+1 555 0100' },
	email: { field: field('email', 'email'), stored: 'a@b.test', draft: 'a@b.test' },
	address: { field: field('addr', 'address'), stored: '123 Test Ave', draft: '123 Test Ave' },
	location: {
		field: field('where', 'location'),
		stored: { lat: 40.7128, lng: -74.006, label: 'Store' },
		draft: '40.7128,-74.006,Store'
	},
	scan: { field: field('sku', 'scan'), stored: 'SKU-1', draft: 'SKU-1' },
	signature: {
		field: field('sig', 'signature'),
		stored: 'data:image/png;base64,abc',
		draft: 'data:image/png;base64,abc'
	},
	autonumber: {
		field: field('no', 'autonumber'),
		stored: 'AUTO-000001',
		draft: 'AUTO-000001',
		context: { existingValue: 'AUTO-000001' }
	},
	member: {
		field: field('owner', 'member'),
		stored: { user_id: 'user-1', name: 'Ada', email: 'ada@example.test' },
		draft: 'user-1'
	},
	textarea: { field: field('body', 'textarea'), stored: 'multi\nline', draft: 'multi\nline' },
	rich_text: { field: field('rich', 'rich_text'), stored: '<p>hi</p>', draft: '<p>hi</p>' },
	number: {
		field: field('qty', 'number'),
		stored: { type: 'decimal', decimal: '12.50' },
		draft: '12.50',
		// Decimals go back as strings: the API rejects JSON numbers.
		submitted: '12.50'
	},
	integer: {
		field: field('count', 'integer'),
		stored: { type: 'integer', value: 7 },
		draft: '7',
		submitted: '7'
	},
	amount: {
		field: field('total', 'amount', { amount: { currency: 'CNY', scale: 2 } }),
		stored: { type: 'amount', decimal: '99.90', currency: 'CNY', scale: 2 },
		draft: '99.90',
		submitted: '99.90'
	},
	rating: {
		field: field('stars', 'rating'),
		stored: { type: 'rating', value: 4 },
		draft: '4',
		submitted: '4'
	},
	progress: {
		field: field('done', 'progress'),
		stored: { type: 'progress', value: 75 },
		draft: '75',
		submitted: '75'
	},
	date: { field: field('day', 'date'), stored: '2026-08-11', draft: '2026-08-11' },
	datetime: {
		field: field('at', 'datetime'),
		stored: '2026-08-11T10:00',
		draft: '2026-08-11T10:00'
	},
	single_select: {
		field: field('status', 'single_select', { options: ['open', 'closed'] }),
		stored: 'open',
		draft: 'open'
	},
	multi_select: {
		field: field('tags', 'multi_select', { options: ['a', 'b'] }),
		stored: ['a', 'b'],
		draft: ['a', 'b']
	},
	boolean: { field: field('flag', 'boolean'), stored: true, draft: true },
	attachment: {
		field: field('file', 'attachment'),
		stored: 'https://cdn.test/a.pdf',
		draft: 'https://cdn.test/a.pdf'
	},
	image: {
		field: field('cover', 'image'),
		stored: 'https://cdn.test/a.png',
		draft: 'https://cdn.test/a.png'
	},
	relation: {
		field: field('order', 'relation', { relation: { form_key: 'orders' } }),
		stored: { record_id: 'rec-1', title: 'Order #1', form_key: 'orders' },
		draft: 'rec-1'
	},
	child_table: {
		field: field('lines', 'child_table'),
		stored: [{ record_id: 'child-1' }],
		draft: [{ record_id: 'child-1' }],
		context: { existingValue: [{ record_id: 'child-1' }] }
	},
	formula: {
		field: field('calc', 'formula'),
		stored: { type: 'decimal', decimal: '3.00' },
		draft: '{"type":"decimal","decimal":"3.00"}',
		submitted: { type: 'decimal', decimal: '3.00' }
	},
	ai_summary: { field: field('summary', 'ai_summary'), stored: 'A summary', draft: 'A summary' }
};

let failures = 0;

function check(name: string, run: () => void) {
	try {
		run();
	} catch (error) {
		failures += 1;
		console.error(`FAIL ${name}`);
		console.error(error instanceof Error ? error.message : error);
	}
}

for (const type of FORM_FIELD_TYPES) {
	const testCase = CASES[type];
	const caseContext = { ...context, ...testCase.context };

	check(`${type}: stored value maps to the editor draft`, () => {
		assert.deepEqual(draftValueFromRecordValue(testCase.field, testCase.stored), testCase.draft);
	});

	check(`${type}: draft maps back to a submittable value`, () => {
		assert.deepEqual(
			recordValueFromDraftValue(testCase.field, testCase.draft, caseContext),
			testCase.submitted ?? testCase.stored
		);
	});

	check(`${type}: draft is a fixed point across a save/reload cycle`, () => {
		const submitted = recordValueFromDraftValue(testCase.field, testCase.draft, caseContext);
		assert.deepEqual(draftValueFromRecordValue(testCase.field, submitted), testCase.draft);
	});

	check(`${type}: an empty draft never fabricates a value`, () => {
		const empty = draftValueFromRecordValue(testCase.field, null);
		const submitted = recordValueFromDraftValue(testCase.field, empty, {});
		if (type === 'boolean') {
			assert.equal(submitted, false);
			return;
		}
		assert.equal(submitted, undefined);
	});
}

check('every field type has a round-trip case', () => {
	assert.deepEqual(Object.keys(CASES).sort(), [...FORM_FIELD_TYPES].sort());
	assert.equal(FORM_FIELD_TYPES.length, 27);
});

check('a required field without a value is reported instead of dropped', () => {
	const required = field('tags', 'multi_select', { options: ['a'], required: true });
	assert.equal(recordValueFromDraftValue(required, [], {}), undefined);
	assert.equal(isMissingRequiredValue(required, undefined), true);
	// Autonumber is generated server side and is exempt.
	assert.equal(
		isMissingRequiredValue(field('no', 'autonumber', { required: true }), undefined),
		false
	);
});

check('multi_select without options round-trips through comma separated text', () => {
	const plain = field('tags', 'multi_select');
	assert.equal(draftValueFromRecordValue(plain, ['a', 'b']), 'a, b');
	assert.deepEqual(recordValueFromDraftValue(plain, 'a, b', {}), ['a', 'b']);
});

check('child_table keeps the stored rollup when the editor has no control for it', () => {
	const lines = field('lines', 'child_table', { required: true });
	const stored = [{ record_id: 'child-1' }];
	assert.deepEqual(recordValueFromDraftValue(lines, '', { existingValue: stored }), stored);
	assert.equal(recordValueFromDraftValue(lines, '', {}), undefined);
});

check('location keeps free text as free text', () => {
	const where = field('where', 'location');
	assert.equal(recordValueFromDraftValue(where, 'Beijing, Chaoyang', {}), 'Beijing, Chaoyang');
	assert.deepEqual(recordValueFromDraftValue(where, '1,2', {}), { lat: 1, lng: 2 });
	assert.equal(recordValueFromDraftValue(where, '999,2', {}), '999,2');
});

if (failures > 0) {
	console.error(`${failures} check(s) failed`);
	process.exit(1);
}

console.log(`record-values round-trip: ${FORM_FIELD_TYPES.length} field types verified`);
