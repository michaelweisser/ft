pub mod json;
pub mod markdown;
pub mod ndjson;
pub mod table;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Table,
    Json,
    Ndjson,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum GroupBy {
    Path,
    Folder,
    Due,
    Priority,
    Tag,
}
