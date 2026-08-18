use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;

const SCHEMA_VERSION: u32 = 2;
const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
const SQLITE_OPEN_FULLMUTEX: c_int = 0x0001_0000;
const SQLITE_TRANSIENT: isize = -1;

#[repr(C)]
struct Sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
struct Sqlite3Stmt {
    _private: [u8; 0],
}

// MSVC에서 기본 `dylib` 링크는 `__imp_sqlite3_*` 심볼을 요구합니다.
// 배포 바이너리는 DLL 없이 실행되도록 Windows에서 vcpkg 정적 SQLite를 연결합니다.
#[cfg_attr(windows, link(name = "sqlite3", kind = "static"))]
#[cfg_attr(not(windows), link(name = "sqlite3"))]
unsafe extern "C" {
    fn sqlite3_open_v2(
        filename: *const c_char,
        database: *mut *mut Sqlite3,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close_v2(database: *mut Sqlite3) -> c_int;
    fn sqlite3_errmsg(database: *mut Sqlite3) -> *const c_char;
    fn sqlite3_exec(
        database: *mut Sqlite3,
        sql: *const c_char,
        callback: Option<unsafe extern "C" fn() -> c_int>,
        argument: *mut c_void,
        error_message: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_prepare_v2(
        database: *mut Sqlite3,
        sql: *const c_char,
        length: c_int,
        statement: *mut *mut Sqlite3Stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_finalize(statement: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_reset(statement: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_clear_bindings(statement: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_bind_text(
        statement: *mut Sqlite3Stmt,
        index: c_int,
        value: *const c_char,
        length: c_int,
        destructor: isize,
    ) -> c_int;
    fn sqlite3_bind_blob(
        statement: *mut Sqlite3Stmt,
        index: c_int,
        value: *const c_void,
        length: c_int,
        destructor: isize,
    ) -> c_int;
    fn sqlite3_step(statement: *mut Sqlite3Stmt) -> c_int;
    fn sqlite3_column_blob(statement: *mut Sqlite3Stmt, column: c_int) -> *const c_void;
    fn sqlite3_column_bytes(statement: *mut Sqlite3Stmt, column: c_int) -> c_int;
    fn sqlite3_column_int64(statement: *mut Sqlite3Stmt, column: c_int) -> i64;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct LegacyDatabaseImage {
    schema_version: u32,
    generation: u64,
    values: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug)]
struct Connection(*mut Sqlite3);

impl Connection {
    fn open(path: &Path) -> Result<Self, String> {
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| "SQLite 경로에 NUL 문자가 있습니다.".to_string())?;
        let mut database = ptr::null_mut();
        // SAFETY: path는 유효한 NUL 종료 문자열이고 database는 sqlite가 채울 포인터입니다.
        let result = unsafe {
            sqlite3_open_v2(
                path.as_ptr(),
                &mut database,
                SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX,
                ptr::null(),
            )
        };
        if result != SQLITE_OK {
            let message = if database.is_null() {
                format!("SQLite 열기 실패(code={result})")
            } else {
                sqlite_error(database)
            };
            if !database.is_null() {
                // SAFETY: sqlite3_open_v2가 생성한 연결을 오류 경로에서 닫습니다.
                unsafe { sqlite3_close_v2(database) };
            }
            return Err(message);
        }
        Ok(Self(database))
    }

    fn execute(&self, sql: &str) -> Result<(), String> {
        let sql = CString::new(sql).map_err(|_| "SQLite SQL에 NUL 문자가 있습니다.".to_string())?;
        // SAFETY: 연결과 SQL 문자열은 이 호출 동안 유효합니다.
        let result =
            unsafe { sqlite3_exec(self.0, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
        if result == SQLITE_OK {
            Ok(())
        } else {
            Err(sqlite_error(self.0))
        }
    }

    fn prepare(&self, sql: &str) -> Result<Statement, String> {
        let sql = CString::new(sql).map_err(|_| "SQLite SQL에 NUL 문자가 있습니다.".to_string())?;
        let mut statement = ptr::null_mut();
        // SAFETY: 연결과 SQL 문자열은 유효하고 sqlite가 statement를 초기화합니다.
        let result = unsafe {
            sqlite3_prepare_v2(self.0, sql.as_ptr(), -1, &mut statement, ptr::null_mut())
        };
        if result == SQLITE_OK {
            Ok(Statement {
                database: self.0,
                statement,
            })
        } else {
            Err(sqlite_error(self.0))
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: 이 Connection이 소유한 sqlite 연결을 한 번 닫습니다.
        unsafe { sqlite3_close_v2(self.0) };
    }
}

struct Statement {
    database: *mut Sqlite3,
    statement: *mut Sqlite3Stmt,
}

impl Statement {
    fn bind_text(&self, index: c_int, value: &str) -> Result<(), String> {
        let length =
            c_int::try_from(value.len()).map_err(|_| "SQLite key가 너무 큽니다.".to_string())?;
        // SAFETY: SQLITE_TRANSIENT로 전달하여 sqlite가 호출 중 문자열을 복사합니다.
        let result = unsafe {
            sqlite3_bind_text(
                self.statement,
                index,
                value.as_ptr().cast(),
                length,
                SQLITE_TRANSIENT,
            )
        };
        self.check(result)
    }

    fn bind_blob(&self, index: c_int, value: &[u8]) -> Result<(), String> {
        let length =
            c_int::try_from(value.len()).map_err(|_| "SQLite value가 너무 큽니다.".to_string())?;
        // SAFETY: SQLITE_TRANSIENT로 전달하여 sqlite가 호출 중 바이트를 복사합니다.
        let result = unsafe {
            sqlite3_bind_blob(
                self.statement,
                index,
                value.as_ptr().cast(),
                length,
                SQLITE_TRANSIENT,
            )
        };
        self.check(result)
    }

    fn step(&self) -> Result<c_int, String> {
        // SAFETY: prepare로 생성되어 finalize 전인 statement입니다.
        let result = unsafe { sqlite3_step(self.statement) };
        if matches!(result, SQLITE_ROW | SQLITE_DONE) {
            Ok(result)
        } else {
            Err(sqlite_error(self.database))
        }
    }

    fn reset(&self) -> Result<(), String> {
        // SAFETY: statement를 다음 batch 항목에 재사용합니다.
        let reset = unsafe { sqlite3_reset(self.statement) };
        self.check(reset)?;
        // SAFETY: reset된 statement의 이전 바인딩을 제거합니다.
        let clear = unsafe { sqlite3_clear_bindings(self.statement) };
        self.check(clear)
    }

    fn blob(&self, column: c_int) -> Vec<u8> {
        // SAFETY: SQLITE_ROW 직후 현재 row의 column을 읽습니다.
        let length = unsafe { sqlite3_column_bytes(self.statement, column) };
        if length <= 0 {
            return Vec::new();
        }
        // SAFETY: sqlite가 length 바이트의 유효한 포인터를 현재 row 동안 제공합니다.
        unsafe {
            let value = sqlite3_column_blob(self.statement, column).cast::<u8>();
            std::slice::from_raw_parts(value, length as usize).to_vec()
        }
    }

    fn check(&self, result: c_int) -> Result<(), String> {
        if result == SQLITE_OK {
            Ok(())
        } else {
            Err(sqlite_error(self.database))
        }
    }
}

impl Drop for Statement {
    fn drop(&mut self) {
        // SAFETY: 이 Statement가 소유한 prepared statement를 한 번 해제합니다.
        unsafe { sqlite3_finalize(self.statement) };
    }
}

fn sqlite_error(database: *mut Sqlite3) -> String {
    // SAFETY: 살아 있는 sqlite 연결은 NUL 종료 오류 문자열을 반환합니다.
    unsafe {
        let message = sqlite3_errmsg(database);
        if message.is_null() {
            "알 수 없는 SQLite 오류".into()
        } else {
            CStr::from_ptr(message).to_string_lossy().into_owned()
        }
    }
}

/// SQLite WAL을 사용하는 실제 영구 key-value 저장소입니다.
///
/// 쓰기는 메모리에 모은 뒤 하나의 `BEGIN IMMEDIATE` transaction으로 반영됩니다.
/// 기존 v0.15.1 JSON image가 있으면 `.legacy-v1.json`으로 보존하고 자동 이관합니다.
#[derive(Clone, Debug)]
pub struct EmbeddedDb {
    path: PathBuf,
    pending: BTreeMap<String, Option<Vec<u8>>>,
}

impl EmbeddedDb {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, String> {
        let directory = data_dir.as_ref().join("db");
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let path = directory.join("ieum-state.sqlite3");
        let legacy_path = directory.join("ieum-state.db");
        let is_new = !path.exists();
        let db = Self {
            path,
            pending: BTreeMap::new(),
        };
        db.initialize()?;
        if is_new && legacy_path.exists() {
            db.import_legacy(&legacy_path)?;
        }
        Ok(db)
    }

    fn initialize(&self) -> Result<(), String> {
        let connection = Connection::open(&self.path)?;
        connection.execute("PRAGMA journal_mode=WAL;")?;
        connection.execute("PRAGMA synchronous=FULL;")?;
        connection.execute("PRAGMA foreign_keys=ON;")?;
        connection.execute("PRAGMA busy_timeout=5000;")?;
        connection.execute(
            "CREATE TABLE IF NOT EXISTS metadata (
                name TEXT PRIMARY KEY NOT NULL,
                value INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS kv (
                key TEXT PRIMARY KEY NOT NULL,
                value BLOB NOT NULL
            );
            INSERT OR IGNORE INTO metadata(name, value) VALUES
                ('schema_version', 2),
                ('generation', 0);",
        )?;
        let schema = self.metadata("schema_version")?;
        if schema != u64::from(SCHEMA_VERSION) {
            return Err(format!("지원하지 않는 SQLite schema입니다: {schema}"));
        }
        Ok(())
    }

    fn import_legacy(&self, legacy_path: &Path) -> Result<(), String> {
        let bytes = fs::read(legacy_path).map_err(|error| error.to_string())?;
        let legacy: LegacyDatabaseImage = serde_json::from_slice(&bytes)
            .map_err(|error| format!("기존 embedded DB 손상: {error}"))?;
        if legacy.schema_version != 1 {
            return Err("지원하지 않는 기존 embedded DB schema입니다.".into());
        }
        let mut migrated = self.clone();
        for (key, value) in legacy.values {
            migrated.put(key, value);
        }
        migrated.commit_with_generation(legacy.generation)?;
        let backup = legacy_path.with_extension("legacy-v1.json");
        fs::rename(legacy_path, backup).map_err(|error| error.to_string())
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        if let Some(value) = self.pending.get(key) {
            return Ok(value.clone());
        }
        let connection = Connection::open(&self.path)?;
        let statement = connection.prepare("SELECT value FROM kv WHERE key = ?1")?;
        statement.bind_text(1, key)?;
        match statement.step()? {
            SQLITE_ROW => Ok(Some(statement.blob(0))),
            SQLITE_DONE => Ok(None),
            _ => unreachable!(),
        }
    }

    pub fn put(&mut self, key: impl Into<String>, value: Vec<u8>) {
        self.pending.insert(key.into(), Some(value));
    }

    pub fn remove(&mut self, key: &str) {
        self.pending.insert(key.to_owned(), None);
    }

    pub fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        let connection = Connection::open(&self.path)?;
        let statement = connection
            .prepare("SELECT key, value FROM kv WHERE key LIKE ?1 ESCAPE '\\' ORDER BY key")?;
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        statement.bind_text(1, &format!("{escaped}%"))?;
        let mut values = Vec::new();
        while statement.step()? == SQLITE_ROW {
            // key는 TEXT이므로 blob 복사 후 UTF-8을 검증합니다.
            let key = String::from_utf8(statement.blob(0))
                .map_err(|_| "SQLite key가 UTF-8이 아닙니다.".to_string())?;
            values.push((key, statement.blob(1)));
        }
        for (key, value) in &self.pending {
            if key.starts_with(prefix) {
                values.retain(|(stored, _)| stored != key);
                if let Some(value) = value {
                    values.push((key.clone(), value.clone()));
                }
            }
        }
        values.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(values)
    }

    pub fn commit(&mut self) -> Result<u64, String> {
        let generation = self.generation()?.saturating_add(1);
        self.commit_with_generation(generation)?;
        Ok(generation)
    }

    fn commit_with_generation(&mut self, generation: u64) -> Result<(), String> {
        let connection = Connection::open(&self.path)?;
        connection.execute("BEGIN IMMEDIATE;")?;
        let result = self.apply_pending(&connection, generation);
        match result {
            Ok(()) => {
                connection.execute("COMMIT;")?;
                self.pending.clear();
                Ok(())
            }
            Err(error) => {
                let _ = connection.execute("ROLLBACK;");
                Err(error)
            }
        }
    }

    fn apply_pending(&self, connection: &Connection, generation: u64) -> Result<(), String> {
        let upsert = connection.prepare(
            "INSERT INTO kv(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )?;
        let delete = connection.prepare("DELETE FROM kv WHERE key = ?1")?;
        for (key, value) in &self.pending {
            if let Some(value) = value {
                upsert.bind_text(1, key)?;
                upsert.bind_blob(2, value)?;
                if upsert.step()? != SQLITE_DONE {
                    return Err("SQLite upsert가 완료되지 않았습니다.".into());
                }
                upsert.reset()?;
            } else {
                delete.bind_text(1, key)?;
                if delete.step()? != SQLITE_DONE {
                    return Err("SQLite delete가 완료되지 않았습니다.".into());
                }
                delete.reset()?;
            }
        }
        connection.execute(&format!(
            "UPDATE metadata SET value = {generation} WHERE name = 'generation';"
        ))
    }

    pub fn generation(&self) -> Result<u64, String> {
        self.metadata("generation")
    }

    fn metadata(&self, name: &str) -> Result<u64, String> {
        let connection = Connection::open(&self.path)?;
        let statement = connection.prepare("SELECT value FROM metadata WHERE name = ?1")?;
        statement.bind_text(1, name)?;
        if statement.step()? != SQLITE_ROW {
            return Err(format!("SQLite metadata가 없습니다: {name}"));
        }
        // SAFETY: SQLITE_ROW 직후 정수 column을 읽습니다.
        let value = unsafe { sqlite3_column_int64(statement.statement, 0) };
        u64::try_from(value).map_err(|_| format!("SQLite metadata가 음수입니다: {name}"))
    }

    pub fn checkpoint(&self) -> Result<(), String> {
        Connection::open(&self.path)?.execute("PRAGMA wal_checkpoint(TRUNCATE);")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        std::env::temp_dir().join(format!(
            "ieum-sqlite-{name}-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn persists_atomic_key_value_transaction() {
        let root = test_root("persist");
        let mut db = EmbeddedDb::open(&root).unwrap();
        db.put("state/root", b"abc".to_vec());
        db.put("state/height", b"1".to_vec());
        assert_eq!(db.commit().unwrap(), 1);
        let restored = EmbeddedDb::open(&root).unwrap();
        assert_eq!(restored.get("state/root").unwrap(), Some(b"abc".to_vec()));
        assert_eq!(restored.generation().unwrap(), 1);
        assert_eq!(restored.scan_prefix("state/").unwrap().len(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn migrates_legacy_json_image_once() {
        let root = test_root("migration");
        let directory = root.join("db");
        fs::create_dir_all(&directory).unwrap();
        let legacy = LegacyDatabaseImage {
            schema_version: 1,
            generation: 7,
            values: BTreeMap::from([("canonical/current".into(), b"legacy".to_vec())]),
        };
        fs::write(
            directory.join("ieum-state.db"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let db = EmbeddedDb::open(&root).unwrap();
        assert_eq!(
            db.get("canonical/current").unwrap(),
            Some(b"legacy".to_vec())
        );
        assert_eq!(db.generation().unwrap(), 7);
        assert!(directory.join("ieum-state.legacy-v1.json").exists());
        let _ = fs::remove_dir_all(root);
    }
}
