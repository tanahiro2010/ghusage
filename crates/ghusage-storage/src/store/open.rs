impl Store {
    /// Opens (creating if needed) the database at `path` and migrates it to
    /// the current schema. Detects corruption and newer schemas.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path).map_err(classify)
    }

    fn open_inner(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            create_private_dir(parent)?;
        }
        check_header(path)?;
        let mut conn = Connection::open(path)?;
        // The cache holds private repository names and titles. SQLite gives
        // its journal files the same permissions as the database.
        restrict_to_user(path, 0o600);
        configure(&conn)?;
        schema::migrate(&mut conn, schema::MIGRATIONS, Some(path))?;
        probe(&conn)?;
        Ok(Self {
            conn,
            path: Some(path.to_path_buf()),
        })
    }

    /// Opens an existing database for reading only, for when the file or
    /// its directory is not writable. Every write through the returned
    /// store fails. The schema must already be current.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        Self::open_read_only_inner(path).map_err(classify)
    }

    fn open_read_only_inner(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Err(StorageError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} does not exist", path.display()),
            )));
        }
        check_header(path)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = match Connection::open_with_flags(path, flags) {
            Ok(c) if Self::readable(&c) => c,
            _ => {
                // A WAL database cannot be read without creating its shared
                // memory file. When no write-ahead log is left over, the main
                // file holds every committed change and can be read as
                // immutable.
                let wal = wal_path(path);
                if std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0) {
                    return Err(StorageError::Data(format!(
                        "{} has unsaved changes in {} and cannot be read without write access",
                        path.display(),
                        wal.display()
                    )));
                }
                Connection::open_with_flags(
                    immutable_uri(path)?,
                    flags | OpenFlags::SQLITE_OPEN_URI,
                )?
            }
        };
        conn.busy_timeout(BUSY_TIMEOUT)?;
        let version = schema::user_version(&conn)?;
        if version > schema::CURRENT_VERSION {
            return Err(StorageError::TooNew {
                found: version,
                supported: schema::CURRENT_VERSION,
            });
        }
        if version < schema::CURRENT_VERSION {
            return Err(StorageError::Migration {
                from: version,
                to: schema::CURRENT_VERSION,
                message: "the cache needs upgrading but is not writable".into(),
            });
        }
        probe(&conn)?;
        Ok(Self {
            conn,
            path: Some(path.to_path_buf()),
        })
    }

    fn readable(conn: &Connection) -> bool {
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .is_ok()
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        schema::migrate(&mut conn, schema::MIGRATIONS, None)?;
        Ok(Self { conn, path: None })
    }

    /// Highest schema version this build can read and write.
    pub fn supported_schema_version() -> u32 {
        schema::CURRENT_VERSION
    }

    pub fn schema_version(&self) -> Result<u32> {
        schema::user_version(&self.conn)
            .map_err(StorageError::from)
            .map_err(classify)
    }

    /// Runs SQLite integrity and foreign-key checks; returns problems found.
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        self.integrity_check_inner().map_err(classify)
    }

    fn integrity_check_inner(&self) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        let mut stmt = self.conn.prepare("PRAGMA quick_check")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            let msg = row?;
            if msg != "ok" {
                problems.push(msg);
            }
        }
        let mut stmt = self.conn.prepare("PRAGMA foreign_key_check")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (table, rowid, parent) = row?;
            let at = rowid.map(|r| format!(" (row {r})")).unwrap_or_default();
            problems.push(format!(
                "foreign key violation in {table}{at}: missing row in {parent}"
            ));
        }
        Ok(problems)
    }

}


