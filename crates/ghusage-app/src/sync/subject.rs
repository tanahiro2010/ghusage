/// Resolves who "you" is: `--user` when given, else the authenticated user.
/// Remembers the viewer for offline use.
pub async fn resolve_subject(
    client: &GitHubClient,
    store: &mut Store,
    user: Option<&str>,
) -> Result<(Subject, bool)> {
    if let Some(login) = user {
        let u = client.user(login).await?;
        Ok((
            Subject {
                id: Some(u.id),
                login: u.login,
            },
            false,
        ))
    } else {
        let v = client.viewer().await?;
        store.set_meta(META_VIEWER_LOGIN, &v.login)?;
        store.set_meta(META_VIEWER_ID, &v.id)?;
        Ok((
            Subject {
                id: Some(v.id),
                login: v.login,
            },
            true,
        ))
    }
}

/// Subject from the cache, for offline use.
pub fn cached_subject(store: &Store, user: Option<&str>) -> Result<(Subject, bool)> {
    let viewer_login = store.meta(META_VIEWER_LOGIN)?;
    match (user, viewer_login) {
        (Some(login), Some(v)) if v.eq_ignore_ascii_case(login) => Ok((
            Subject {
                id: store.meta(META_VIEWER_ID)?,
                login: v,
            },
            true,
        )),
        (Some(login), _) => Ok((
            Subject {
                id: None,
                login: login.to_string(),
            },
            false,
        )),
        (None, Some(v)) => Ok((
            Subject {
                id: store.meta(META_VIEWER_ID)?,
                login: v,
            },
            true,
        )),
        (None, None) => Err(AppError::Other(
            "no cached data for the authenticated user; run `ghusage sync` online first or pass --user"
                .into(),
        )),
    }
}
