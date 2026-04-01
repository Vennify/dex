use tantivy::schema::*;
use tantivy::{Index, IndexWriter, TantivyDocument};
use tantivy::directory::MmapDirectory;

use std::path::Path;

use crate::parse::Record;

/// Build the tantivy schema matching the spec.
pub fn build_schema() -> Schema {
    let mut builder = Schema::builder();

    // Identity
    builder.add_text_field("session_id", STRING | STORED);
    builder.add_text_field("message_id", STRING | STORED);

    // Taxonomy
    builder.add_text_field("project", STRING | STORED);
    builder.add_text_field("role", STRING | STORED);
    builder.add_text_field("content_type", STRING | STORED);
    builder.add_text_field("tool_name", STRING | STORED);

    // Extracted fields
    builder.add_text_field("file_path", TEXT | STORED);
    builder.add_text_field("command", TEXT | STORED);

    // Content — full-text indexed
    builder.add_text_field("content", TEXT | STORED);

    // Ordering
    builder.add_date_field("timestamp", INDEXED | STORED | FAST);
    builder.add_u64_field("sequence", INDEXED | STORED | FAST);

    builder.build()
}

/// Open or create a tantivy index at the given path.
pub fn open_or_create(path: &Path, schema: &Schema) -> tantivy::Result<Index> {
    std::fs::create_dir_all(path).map_err(|e| tantivy::TantivyError::IoError(e.into()))?;
    let dir = MmapDirectory::open(path)?;
    Index::open_or_create(dir, schema.clone())
}

/// Index a batch of records into tantivy. Returns number of documents added.
pub fn index_records(writer: &IndexWriter, schema: &Schema, records: &[Record]) -> tantivy::Result<u64> {
    let session_id_field = schema.get_field("session_id").unwrap();
    let message_id_field = schema.get_field("message_id").unwrap();
    let project_field = schema.get_field("project").unwrap();
    let role_field = schema.get_field("role").unwrap();
    let content_type_field = schema.get_field("content_type").unwrap();
    let tool_name_field = schema.get_field("tool_name").unwrap();
    let file_path_field = schema.get_field("file_path").unwrap();
    let command_field = schema.get_field("command").unwrap();
    let content_field = schema.get_field("content").unwrap();
    let timestamp_field = schema.get_field("timestamp").unwrap();
    let sequence_field = schema.get_field("sequence").unwrap();

    let mut count = 0u64;

    for record in records {
        let mut doc = TantivyDocument::new();
        doc.add_text(session_id_field, &record.session_id);
        doc.add_text(message_id_field, &record.message_id);
        doc.add_text(project_field, &record.project);
        doc.add_text(role_field, record.role.as_str());
        doc.add_text(content_type_field, record.content_type.as_str());
        doc.add_text(tool_name_field, record.tool_name.as_deref().unwrap_or(""));
        doc.add_text(file_path_field, record.file_path.as_deref().unwrap_or(""));
        doc.add_text(command_field, record.command.as_deref().unwrap_or(""));
        doc.add_text(content_field, &record.content);

        if let Some(ts) = record.timestamp {
            let dt = tantivy::DateTime::from_timestamp_secs(ts.timestamp());
            doc.add_date(timestamp_field, dt);
        }
        doc.add_u64(sequence_field, record.sequence);

        writer.add_document(doc)?;
        count += 1;
    }

    Ok(count)
}

/// Delete all documents for a given session (used before re-indexing a changed session).
pub fn delete_session(writer: &IndexWriter, schema: &Schema, session_id: &str) {
    let session_id_field = schema.get_field("session_id").unwrap();
    let term = tantivy::Term::from_field_text(session_id_field, session_id);
    writer.delete_term(term);
}
