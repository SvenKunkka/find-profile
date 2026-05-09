use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
#[error("invalid enum value: {0}")]
pub struct EnumParseError(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RootKind {
    Local,
    Nas,
}

impl RootKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Nas => "nas",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "local" => Some(Self::Local),
            "nas" => Some(Self::Nas),
            _ => None,
        }
    }

    pub fn detect_from_path(path: &str) -> Self {
        if path.starts_with("/Volumes/") {
            Self::Nas
        } else {
            Self::Local
        }
    }
}

impl FromSql for RootKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::parse(s).ok_or_else(|| {
            FromSqlError::Other(Box::new(EnumParseError(format!("RootKind: {s}"))))
        })
    }
}

impl ToSql for RootKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RootStatus {
    Available,
    Disconnected,
    Scanning,
    Error,
}

impl RootStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Disconnected => "disconnected",
            Self::Scanning => "scanning",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "available" => Some(Self::Available),
            "disconnected" => Some(Self::Disconnected),
            "scanning" => Some(Self::Scanning),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

impl FromSql for RootStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::parse(s).ok_or_else(|| {
            FromSqlError::Other(Box::new(EnumParseError(format!("RootStatus: {s}"))))
        })
    }
}

impl ToSql for RootStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanJobStatus {
    Running,
    Success,
    Failed,
    Cancelled,
}

impl ScanJobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "success" => Some(Self::Success),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

impl FromSql for ScanJobStatus {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = value.as_str()?;
        Self::parse(s).ok_or_else(|| {
            FromSqlError::Other(Box::new(EnumParseError(format!("ScanJobStatus: {s}"))))
        })
    }
}

impl ToSql for ScanJobStatus {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(self.as_str().into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Root {
    pub id: i64,
    pub path: String,
    pub kind: RootKind,
    pub enabled: bool,
    pub status: RootStatus,
    pub scan_interval: Option<String>,
    pub last_scanned_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub root_id: i64,
    pub path: String,
    pub name: String,
    pub extension: Option<String>,
    pub size: Option<i64>,
    pub modified_at: Option<String>,
    pub is_dir: bool,
    pub source_kind: RootKind,
    pub is_accessible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHit {
    pub id: i64,
    pub root_id: i64,
    pub path: String,
    pub name: String,
    pub extension: Option<String>,
    pub size: Option<i64>,
    pub modified_at: Option<String>,
    pub is_dir: bool,
    pub source_kind: RootKind,
    pub is_accessible: bool,
    pub indexed_at: String,
    /// Live status of the root that owns this hit. The frontend uses this to
    /// render Accessible as `unavailable` when the root is disconnected, even
    /// if the file's own `is_accessible` was true at scan time.
    pub root_status: RootStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanJob {
    pub id: i64,
    pub root_id: i64,
    pub status: ScanJobStatus,
    pub scanned_count: i64,
    pub error_count: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error_message: Option<String>,
}
