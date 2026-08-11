/**
 * Bidirectional mapping between the flat record editor draft (mostly strings,
 * because every editor input is a DOM control) and the structured record
 * values the forms API stores.
 *
 * Why this has to be exhaustive: `PATCH /api/v1/form-records/:id` replaces the
 * whole `values` map instead of merging it (see `apps/api/src/forms/values.rs`,
 * `normalize_record_values_with_existing` rebuilds an empty map). Any field the
 * frontend fails to put back into the payload is therefore permanently deleted
 * from the stored record. `FIELD_CODECS` is keyed by `FormFieldType`, so adding
 * a field type to the union without adding a codec is a compile error rather
 * than a silent data loss bug.
 *
 * Decimal-ish types (`amount`, `number`) are always submitted as strings: the
 * backend rejects JSON numbers for them ("must be a decimal string, not a JSON
 * number").
 */

import type { FormField, FormFieldType } from '$lib/api/forms';

export interface RelationTargetInfo {
	title?: string;
	display?: string;
	form_key?: string;
}

export interface MemberInfo {
	name?: string;
	email?: string;
}

export interface DraftToValueContext {
	/** Resolves the display metadata of a relation target record. */
	relationTarget?: (field: FormField, recordId: string) => RelationTargetInfo | undefined;
	/** Resolves the display metadata of a workspace member. */
	member?: (userId: string) => MemberInfo | undefined;
	/**
	 * Value currently stored on the server for this field. Used by field types
	 * that have no editor control (`child_table`, `autonumber`) so that a save
	 * does not wipe them under the replace-all semantics.
	 */
	existingValue?: unknown;
}

interface FieldCodec {
	/** Stored value -> editor draft. */
	toDraft(field: FormField, value: unknown): unknown;
	/** Editor draft -> stored value. `undefined` means "omit from payload". */
	toValue(field: FormField, draft: unknown, context: DraftToValueContext): unknown;
	/** Draft used when the record has no value for the field. */
	emptyDraft(field: FormField): unknown;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function trimmedText(value: unknown): string {
	if (typeof value === 'string') return value.trim();
	if (typeof value === 'number' || typeof value === 'boolean') return String(value);
	return '';
}

function scalarToDraft(value: unknown): string {
	if (value === null || value === undefined) return '';
	if (typeof value === 'string') return value;
	if (typeof value === 'number' || typeof value === 'boolean') return String(value);
	if (Array.isArray(value)) return value.map((item) => scalarToDraft(item)).join(', ');
	return JSON.stringify(value) ?? '';
}

function jsonishToDraft(value: unknown): string {
	if (value === null || value === undefined) return '';
	if (typeof value === 'string') return value;
	return JSON.stringify(value) ?? '';
}

/** Parses JSON object/array text, leaving anything else as plain text. */
export function parseJsonish(text: string): unknown {
	if (!text.startsWith('{') && !text.startsWith('[')) return text;
	try {
		return JSON.parse(text) as unknown;
	} catch {
		return text;
	}
}

function stringList(value: unknown): string[] {
	if (Array.isArray(value)) {
		return value
			.filter((item): item is string => typeof item === 'string')
			.map((item) => item.trim())
			.filter(Boolean);
	}
	if (typeof value !== 'string') return [];
	return value
		.split(',')
		.map((item) => item.trim())
		.filter(Boolean);
}

function hasOptions(field: FormField): boolean {
	return (field.options?.length ?? 0) > 0;
}

/** `lat,lng` or `lat,lng,label` — the label may itself contain commas. */
export function formatLocationDraft(value: Record<string, unknown>): string {
	const { lat, lng, label } = value;
	if (typeof lat !== 'number' || typeof lng !== 'number') return jsonishToDraft(value);
	const base = `${lat},${lng}`;
	return typeof label === 'string' && label.trim() ? `${base},${label.trim()}` : base;
}

/** Inverse of {@link formatLocationDraft}; free text stays free text. */
export function parseLocationDraft(text: string): unknown {
	const parts = text.split(',');
	if (parts.length < 2) return text;
	const rawLat = (parts[0] ?? '').trim();
	const rawLng = (parts[1] ?? '').trim();
	if (!rawLat || !rawLng) return text;
	const lat = Number(rawLat);
	const lng = Number(rawLng);
	if (!Number.isFinite(lat) || !Number.isFinite(lng)) return text;
	if (lat < -90 || lat > 90 || lng < -180 || lng > 180) return text;
	const label = parts.slice(2).join(',').trim();
	return label ? { lat, lng, label } : { lat, lng };
}

const textCodec: FieldCodec = {
	toDraft: (_field, value) => scalarToDraft(value),
	toValue: (_field, draft) => trimmedText(draft) || undefined,
	emptyDraft: () => ''
};

/** Decimal-ish values must stay strings on the wire. */
const numericCodec: FieldCodec = {
	toDraft: (_field, value) => {
		if (isPlainObject(value)) {
			if (typeof value.decimal === 'string') return value.decimal;
			if (typeof value.value === 'number') return String(value.value);
			return jsonishToDraft(value);
		}
		return scalarToDraft(value);
	},
	toValue: (_field, draft) => trimmedText(draft) || undefined,
	emptyDraft: () => ''
};

const booleanCodec: FieldCodec = {
	toDraft: (_field, value) => Boolean(value),
	toValue: (_field, draft) => Boolean(draft),
	emptyDraft: () => false
};

const singleSelectCodec: FieldCodec = {
	toDraft: (_field, value) => scalarToDraft(value),
	toValue: (_field, draft) => trimmedText(draft) || undefined,
	emptyDraft: () => ''
};

/**
 * Option-backed multi selects use a checkbox group (array draft); option-less
 * ones fall back to a comma separated text input (string draft).
 */
const multiSelectCodec: FieldCodec = {
	toDraft: (field, value) => {
		const items = stringList(value);
		return hasOptions(field) ? items : items.join(', ');
	},
	toValue: (_field, draft) => {
		const items = stringList(draft);
		return items.length > 0 ? items : undefined;
	},
	emptyDraft: (field) => (hasOptions(field) ? [] : '')
};

const relationCodec: FieldCodec = {
	toDraft: (_field, value) => {
		if (isPlainObject(value)) return typeof value.record_id === 'string' ? value.record_id : '';
		return scalarToDraft(value);
	},
	toValue: (field, draft, context) => {
		const recordId = isPlainObject(draft) ? trimmedText(draft.record_id) : trimmedText(draft);
		if (!recordId) return undefined;
		const target = context.relationTarget?.(field, recordId);
		return {
			record_id: recordId,
			title: target?.display || target?.title || recordId,
			form_key: target?.form_key ?? field.relation?.form_key
		};
	},
	emptyDraft: () => ''
};

const memberCodec: FieldCodec = {
	toDraft: (_field, value) => {
		if (isPlainObject(value)) return typeof value.user_id === 'string' ? value.user_id : '';
		return scalarToDraft(value);
	},
	toValue: (_field, draft, context) => {
		const userId = isPlainObject(draft) ? trimmedText(draft.user_id) : trimmedText(draft);
		if (!userId) return undefined;
		const member = context.member?.(userId);
		return {
			user_id: userId,
			...(member?.name ? { name: member.name } : {}),
			...(member?.email ? { email: member.email } : {})
		};
	},
	emptyDraft: () => ''
};

const locationCodec: FieldCodec = {
	toDraft: (_field, value) =>
		isPlainObject(value) ? formatLocationDraft(value) : scalarToDraft(value),
	toValue: (_field, draft) => {
		if (isPlainObject(draft)) return draft;
		const text = trimmedText(draft);
		return text ? parseLocationDraft(text) : undefined;
	},
	emptyDraft: () => ''
};

/** Attachment/image/formula payloads round-trip through their JSON text form. */
const jsonishCodec: FieldCodec = {
	toDraft: (_field, value) => jsonishToDraft(value),
	toValue: (_field, draft) => {
		if (draft !== null && typeof draft === 'object') return draft;
		const text = trimmedText(draft);
		return text ? parseJsonish(text) : undefined;
	},
	emptyDraft: () => ''
};

/**
 * The parent record carries a rollup of its child rows but has no editor for
 * it, so the stored value is echoed back verbatim. Dropping it would delete the
 * rollup, and omitting it makes a required child table unsavable.
 */
const childTableCodec: FieldCodec = {
	toDraft: (_field, value) => value ?? '',
	toValue: (_field, draft, context) => {
		const existing = context.existingValue;
		if (existing !== undefined && existing !== null && existing !== '') return existing;
		if (draft === undefined || draft === null || draft === '') return undefined;
		if (Array.isArray(draft) && draft.length === 0) return undefined;
		return draft;
	},
	emptyDraft: () => ''
};

/** Server generated; never submitted by the editor, but must not be wiped. */
const autonumberCodec: FieldCodec = {
	toDraft: (_field, value) => scalarToDraft(value),
	toValue: (_field, draft, context) => {
		const existing = trimmedText(context.existingValue);
		if (existing) return existing;
		return trimmedText(draft) || undefined;
	},
	emptyDraft: () => ''
};

/**
 * Exhaustive by construction: a new `FormFieldType` without an entry here fails
 * `bun run check`.
 */
const FIELD_CODECS: Record<FormFieldType, FieldCodec> = {
	text: textCodec,
	phone: textCodec,
	email: textCodec,
	address: textCodec,
	location: locationCodec,
	scan: textCodec,
	signature: textCodec,
	autonumber: autonumberCodec,
	member: memberCodec,
	textarea: textCodec,
	rich_text: textCodec,
	number: numericCodec,
	integer: numericCodec,
	amount: numericCodec,
	rating: numericCodec,
	progress: numericCodec,
	date: textCodec,
	datetime: textCodec,
	single_select: singleSelectCodec,
	multi_select: multiSelectCodec,
	boolean: booleanCodec,
	attachment: jsonishCodec,
	image: jsonishCodec,
	relation: relationCodec,
	child_table: childTableCodec,
	formula: jsonishCodec,
	ai_summary: textCodec
};

export const FORM_FIELD_TYPES = Object.keys(FIELD_CODECS) as FormFieldType[];

function codecFor(field: FormField): FieldCodec {
	const type = field.type ?? 'text';
	return FIELD_CODECS[type] ?? textCodec;
}

/** Stored record value -> editor draft value. */
export function draftValueFromRecordValue(field: FormField, value: unknown): unknown {
	const codec = codecFor(field);
	if (value === undefined || value === null) return codec.emptyDraft(field);
	return codec.toDraft(field, value);
}

/** Draft value for a field the record has no value for. */
export function emptyDraftValue(field: FormField): unknown {
	return codecFor(field).emptyDraft(field);
}

/**
 * Editor draft value -> stored record value. Returns `undefined` when the field
 * carries no value and must be omitted from the payload.
 */
export function recordValueFromDraftValue(
	field: FormField,
	draft: unknown,
	context: DraftToValueContext = {}
): unknown {
	return codecFor(field).toValue(field, draft, context);
}

/** True when omitting the field from the payload would fail backend validation. */
export function isMissingRequiredValue(field: FormField, value: unknown): boolean {
	if (!field.required) return false;
	if ((field.type ?? 'text') === 'autonumber') return false;
	return value === undefined;
}
