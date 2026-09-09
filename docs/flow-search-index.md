# Flow search index

Sylvode Flow v0.5 uses a worker-owned PostgreSQL full-text table rather than placing a generated
index directly on `flow_object_projections`. The source table is the accepted read model and is the
only content source: the worker never consumes client-pending updates, event payloads, snapshots,
or CRDT update bytes.

Each search row records the exact accepted `document_seq` and `document_frontier` it copied. This
separation makes search lag observable: `collab_documents.head_seq` is the accepted document head,
`flow_object_projections.document_seq` is the accepted projection head, and
`flow_search_index.indexed_seq` is what the asynchronous worker has indexed. Rebuilds may replace
an accepted projection with an older sequence, so the worker replaces rows on any frontier or
content difference rather than assuming that the source sequence only increases.

The generated `tsvector` uses PostgreSQL's `simple` configuration and weights title tokens above
body tokens. This is a deliberate multilingual baseline: it avoids applying an incorrect
language-specific stemmer across mixed-language workspaces. The cost is weaker stemming and word
segmentation for some languages. A GIN index provides fast matching at the cost of rebuildable
storage and write amplification.

Archived objects are removed by the worker and excluded by the API even before cleanup runs.
Restored objects are rebuilt from their accepted projection. Deleted objects disappear through the
search table's `ON DELETE CASCADE` foreign key.

The response `index_frontier` is policy-filtered. It is aggregated only from objects in the
declared request scope that pass request-time object authorization; inaccessible objects cannot
change its sequence, lag, or stale flag. The aggregate uses sums of per-object indexed and head
sequences, so `lag = head_seq - indexed_seq` remains meaningful across a multi-object scope.
Objects without an index row make the frontier stale even when both sequences are zero. A fixed
authorized-scan budget bounds both frontier evaluation and result overfetch.
