//! The portals derisk answers with its own dialogs: `FileChooser`,
//! `Access` and `AppChooser` (OpenURI's "Open With").

use ashpd::{
    MaybeAppID, PortalError, Uri, WindowIdentifierType,
    async_trait::async_trait,
    backend::{
        Result,
        access::{AccessImpl, AccessOptions, AccessResponse},
        app_chooser::{AppChooserImpl, Choice, ChooserOptions},
        file_chooser::FileChooserImpl,
        request::RequestImpl,
    },
    desktop::{
        HandleToken, Icon,
        file_chooser::{
            FileFilter, OpenFileOptions, SaveFileOptions, SaveFilesOptions, SelectedFiles,
        },
    },
};
use derisk_portal_ui::{Reply, Request, Update, access, app_chooser, file_chooser};
use std::path::{Path, PathBuf};

use crate::{apps, dialog::Dialogs, files};

fn failed(error: impl std::fmt::Display) -> PortalError {
    PortalError::Failed(error.to_string())
}

/// The app ID as a string, empty for a host app.
pub fn id(app_id: Option<&MaybeAppID>) -> String {
    app_id.map(ToString::to_string).unwrap_or_default()
}

/// Shows `request`; a dismissed dialog is [`PortalError::Cancelled`].
pub async fn ask(dialogs: &Dialogs, token: &HandleToken, request: Request) -> Result<Reply> {
    match dialogs.ask(&token.to_string(), &request).await {
        Ok(Reply::Cancelled) => Err(PortalError::Cancelled("dismissed".into())),
        Ok(reply) => Ok(reply),
        Err(e) => {
            eprintln!("xdg-desktop-portal-derisk: {e}");
            Err(PortalError::Failed(e))
        }
    }
}

fn filter(f: &FileFilter) -> file_chooser::Filter {
    let patterns = f
        .pattern_filters()
        .into_iter()
        .map(|g| file_chooser::Pattern::Glob(g.to_owned()))
        .chain(
            f.mimetype_filters()
                .into_iter()
                .map(|m| file_chooser::Pattern::Mime(m.to_owned())),
        )
        .collect();
    file_chooser::Filter {
        name: f.label().to_owned(),
        patterns,
    }
}

/// The dialog's filters and the index of the current one, which is added
/// when the app's list doesn't contain it.
pub fn filters(
    list: &[FileFilter],
    current: Option<&FileFilter>,
) -> (Vec<file_chooser::Filter>, Option<usize>) {
    let mut out: Vec<_> = list.iter().map(filter).collect();
    let current = current.map(|c| {
        let c = filter(c);
        out.iter().position(|f| *f == c).unwrap_or_else(|| {
            out.push(c);
            out.len() - 1
        })
    });
    (out, current)
}

fn title(title: &str, fallback: &str) -> String {
    if title.is_empty() {
        fallback.to_owned()
    } else {
        title.to_owned()
    }
}

/// The chosen paths as the portal's result.
pub fn selected(paths: &[PathBuf]) -> Result<SelectedFiles> {
    paths
        .iter()
        .try_fold(SelectedFiles::default(), |files, path| {
            Ok(files.uri(Uri::parse(&files::uri_from_path(path)).map_err(failed)?))
        })
}

/// The display name of the app asking.
pub async fn app_name(app_id: Option<&MaybeAppID>) -> String {
    let id = id(app_id);
    tokio::task::spawn_blocking(move || apps::display_name(&apps::entries(), &id))
        .await
        .unwrap_or_else(|_| "An app".into())
}

/// The derisk dialogs, shared by every interface they serve.
#[derive(Clone, Default)]
pub struct Portals {
    /// The dialogs open now.
    pub dialogs: Dialogs,
}

impl Portals {
    async fn files(
        &self,
        token: &HandleToken,
        request: file_chooser::Request,
    ) -> Result<Vec<PathBuf>> {
        match ask(&self.dialogs, token, Request::FileChooser(request)).await? {
            Reply::Files(choice) => Ok(choice.paths),
            _ => Err(failed("the file chooser gave an unexpected answer")),
        }
    }
}

#[async_trait]
impl RequestImpl for Portals {
    // ashpd aborts the request's future, which kills the dialog.
    async fn close(&self, _token: HandleToken) {}
}

#[async_trait]
impl FileChooserImpl for Portals {
    async fn open_file(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        title_text: &str,
        options: OpenFileOptions,
    ) -> Result<SelectedFiles> {
        let (filters, current_filter) = filters(options.filters(), options.current_filter());
        let request = file_chooser::Request {
            app_name: app_name(app_id.as_ref()).await,
            title: title(title_text, "Open File"),
            accept_label: options
                .accept_label()
                .filter(|l| !l.is_empty())
                .map(Into::into),
            multiple: options.multiple().unwrap_or(false),
            directory: options.directory().unwrap_or(false),
            save_name: None,
            filters,
            current_filter,
            current_folder: options.current_folder().map(|p| p.as_ref().to_owned()),
        };
        selected(&self.files(&token, request).await?)
    }

    async fn save_file(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        title_text: &str,
        options: SaveFileOptions,
    ) -> Result<SelectedFiles> {
        let (filters, current_filter) = filters(options.filters(), options.current_filter());
        let current_file: Option<&Path> = options.current_file().map(AsRef::as_ref);
        let name = options
            .current_name()
            .map(ToOwned::to_owned)
            .or_else(|| {
                current_file.and_then(|f| Some(f.file_name()?.to_string_lossy().into_owned()))
            })
            .unwrap_or_default();
        let folder = options
            .current_folder()
            .map(|p| p.as_ref().to_owned())
            .or_else(|| current_file.and_then(|f| f.parent().map(Path::to_owned)));
        let request = file_chooser::Request {
            app_name: app_name(app_id.as_ref()).await,
            title: title(title_text, "Save File"),
            accept_label: options
                .accept_label()
                .filter(|l| !l.is_empty())
                .map(Into::into),
            multiple: false,
            directory: false,
            save_name: Some(name),
            filters,
            current_filter,
            current_folder: folder,
        };
        selected(&self.files(&token, request).await?)
    }

    async fn save_files(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        title_text: &str,
        options: SaveFilesOptions,
    ) -> Result<SelectedFiles> {
        // The person picks a folder; the files keep their names in it.
        let request = file_chooser::Request {
            app_name: app_name(app_id.as_ref()).await,
            title: title(title_text, "Save Files"),
            accept_label: Some(
                options
                    .accept_label()
                    .filter(|l| !l.is_empty())
                    .unwrap_or("Save Here")
                    .to_owned(),
            ),
            multiple: false,
            directory: true,
            save_name: None,
            filters: Vec::new(),
            current_filter: None,
            current_folder: options.current_folder().map(|p| p.as_ref().to_owned()),
        };
        let Some(folder) = self.files(&token, request).await?.into_iter().next() else {
            return Err(failed("no folder chosen"));
        };
        let paths: Vec<PathBuf> = options
            .files()
            .iter()
            .filter_map(|f| AsRef::<Path>::as_ref(f).file_name())
            .map(|name| folder.join(name))
            .collect();
        selected(&paths)
    }
}

/// A window title for an access question, from its wording.
pub fn access_title(title: &str) -> String {
    let lower = title.to_lowercase();
    for (word, name) in [
        ("camera", "Camera Access"),
        ("microphone", "Microphone Access"),
        ("location", "Location Access"),
        ("screenshot", "Screenshot Access"),
        ("background", "Background Access"),
        ("notification", "Notifications"),
    ] {
        if lower.contains(word) {
            return name.into();
        }
    }
    "Access Request".into()
}

/// The access dialog for `app_id` asking `title`.
pub async fn access_request(
    app_id: &str,
    title: String,
    subtitle: String,
    body: String,
    icon: Option<String>,
) -> access::Request {
    let id = app_id.to_owned();
    let (name, entry_icon, sandboxed) = tokio::task::spawn_blocking(move || {
        let entries = apps::entries();
        let icon = apps::find(&entries, &id)
            .map(|e| e.icon.clone())
            .unwrap_or_default();
        (
            apps::display_name(&entries, &id),
            icon,
            apps::is_flatpak(&id),
        )
    })
    .await
    .unwrap_or_else(|_| ("An app".into(), String::new(), false));
    access::Request {
        window_title: access_title(&title),
        app_id: app_id.to_owned(),
        app_name: name,
        app_icon: icon.filter(|i| !i.is_empty()).unwrap_or(entry_icon),
        sandboxed,
        title,
        subtitle,
        body,
        deny_label: None,
        grant_label: None,
        choices: Vec::new(),
    }
}

#[async_trait]
impl AccessImpl for Portals {
    async fn access_dialog(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        title: String,
        subtitle: String,
        body: String,
        options: AccessOptions,
    ) -> Result<AccessResponse> {
        let icon = match options.icon() {
            Some(Icon::Names(names)) => names.into_iter().next(),
            _ => None,
        };
        let mut request = access_request(&id(app_id.as_ref()), title, subtitle, body, icon).await;
        request.deny_label = options
            .deny_label()
            .filter(|l| !l.is_empty())
            .map(Into::into);
        request.grant_label = options
            .grant_label()
            .filter(|l| !l.is_empty())
            .map(Into::into);
        request.choices = options
            .choices()
            .iter()
            .map(|c| access::AccessChoice {
                id: c.id().to_owned(),
                label: c.label().to_owned(),
                options: c
                    .pairs()
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
                initial: c.initial_selection().to_owned(),
            })
            .collect();
        match ask(&self.dialogs, &token, Request::Access(request)).await? {
            Reply::Access(choice) => Ok(choice
                .choices
                .iter()
                .fold(AccessResponse::default(), |r, (k, v)| r.choice(k, v))),
            _ => Err(failed("the access dialog gave an unexpected answer")),
        }
    }
}

/// The apps on offer, the last choice first and marked as the default.
pub fn app_entries(
    ids: &[String],
    last: Option<&str>,
    entries: &[derisk::desktop::DesktopEntry],
) -> Vec<app_chooser::AppEntry> {
    let mut out: Vec<app_chooser::AppEntry> = ids
        .iter()
        .map(|id| app_chooser::AppEntry::from_desktop(id, entries))
        .collect();
    if let Some(at) = last.and_then(|l| out.iter().position(|a| a.id == l)) {
        let mut app = out.remove(at);
        app.detail = "Default".into();
        out.insert(0, app);
    }
    out
}

/// The file name to show for an app chooser request.
pub fn opened_name(filename: Option<&str>, uri: Option<&str>) -> String {
    filename
        .filter(|f| !f.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            let uri = uri?.trim_end_matches('/');
            let last = uri.rsplit('/').next().unwrap_or(uri);
            Some(
                files::path_from_uri(&format!("file:///{last}")).map_or_else(
                    || last.to_owned(),
                    |p| p.to_string_lossy().trim_start_matches('/').to_owned(),
                ),
            )
        })
        .unwrap_or_else(|| "this file".into())
}

#[async_trait]
impl AppChooserImpl for Portals {
    async fn choose_application(
        &self,
        token: HandleToken,
        app_id: Option<MaybeAppID>,
        _parent_window: Option<WindowIdentifierType>,
        choices: Vec<MaybeAppID>,
        options: ChooserOptions,
    ) -> Result<Choice> {
        let ids: Vec<String> = choices.iter().map(ToString::to_string).collect();
        let last = options
            .last_choice()
            .map(ToString::to_string)
            .filter(|l| !l.is_empty());
        let content_type = options.content_type().unwrap_or_default().to_owned();
        let caller = id(app_id.as_ref());
        let (entries, who) = tokio::task::spawn_blocking(move || {
            let entries = apps::entries();
            let who = apps::display_name(&entries, &caller);
            (entries, who)
        })
        .await
        .map_err(failed)?;
        let (one, many) = if content_type.is_empty() {
            ("File".to_owned(), String::new())
        } else {
            apps::type_names(&content_type)
        };
        let request = app_chooser::Request {
            file_name: opened_name(options.filename(), options.uri().map(Uri::as_str)),
            detail: format!("{one} · requested by {who}"),
            content_type: content_type.clone(),
            type_name: many,
            choices: app_entries(&ids, last.as_deref(), &entries),
            last_choice: last,
        };
        match ask(&self.dialogs, &token, Request::AppChooser(request)).await? {
            Reply::App(choice) => {
                if choice.always && !content_type.is_empty() {
                    apps::make_default(&choice.app_id, &content_type).await;
                }
                Ok(Choice::new(MaybeAppID::from(choice.app_id))
                    .activation_token(options.activation_token().cloned()))
            }
            _ => Err(failed("the app chooser gave an unexpected answer")),
        }
    }

    async fn update_choices(&self, token: HandleToken, choices: Vec<MaybeAppID>) -> Result<()> {
        let ids: Vec<String> = choices.iter().map(ToString::to_string).collect();
        let entries = tokio::task::spawn_blocking(apps::entries)
            .await
            .map_err(failed)?;
        let update = Update::Choices {
            choices: app_entries(&ids, None, &entries),
        };
        self.dialogs.update(&token.to_string(), &update).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_keep_the_apps_and_add_a_missing_current_one() {
        let pdf = FileFilter::new("PDF").glob("*.pdf");
        let images = FileFilter::new("Images").mimetype("image/*");
        let (list, current) = filters(&[images.clone(), pdf.clone()], Some(&pdf));
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[0].patterns,
            [file_chooser::Pattern::Mime("image/*".into())]
        );
        assert_eq!(current, Some(1));
        let other = FileFilter::new("Text").glob("*.txt");
        let (list, current) = filters(&[pdf], Some(&other));
        assert_eq!((list.len(), current), (2, Some(1)));
    }

    #[test]
    fn access_titles_follow_the_question() {
        assert_eq!(
            access_title("Allow Calls to use the camera?"),
            "Camera Access"
        );
        assert_eq!(access_title("Take a screenshot?"), "Screenshot Access");
        assert_eq!(access_title("Something else?"), "Access Request");
    }

    #[test]
    fn the_last_choice_comes_first_as_the_default() {
        let ids = ["org.a.A".to_owned(), "org.b.B".to_owned()];
        let apps = app_entries(&ids, Some("org.b.B"), &[]);
        assert_eq!(apps[0].id, "org.b.B");
        assert_eq!(apps[0].detail, "Default");
        assert_eq!(apps[1].name, "A");
    }

    #[test]
    fn opened_names_come_from_the_file_name_or_the_uri() {
        assert_eq!(opened_name(Some("Q3.pdf"), None), "Q3.pdf");
        assert_eq!(
            opened_name(None, Some("file:///home/me/Q3%20report.pdf")),
            "Q3 report.pdf"
        );
        assert_eq!(
            opened_name(None, Some("https://example.org/")),
            "example.org"
        );
        assert_eq!(opened_name(None, None), "this file");
    }

    #[test]
    fn selected_paths_become_file_uris() {
        let files = selected(&[PathBuf::from("/tmp/a b.txt")]).unwrap();
        assert_eq!(files.uris()[0].as_str(), "file:///tmp/a%20b.txt");
    }
}
