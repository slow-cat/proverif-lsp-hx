use dashmap::DashMap;
use tower_lsp::lsp_types::Url;

#[derive(Clone, Debug)]
pub struct DocumentSnapshot {
    #[allow(dead_code)]
    pub version: i32,
    pub text: String,
}

#[derive(Default)]
pub struct DocumentStore {
    docs: DashMap<Url, DocumentSnapshot>,
}

impl DocumentStore {
    pub fn open(&self, uri: Url, version: i32, text: String) {
        self.docs.insert(uri, DocumentSnapshot { version, text });
    }

    pub fn update(&self, uri: &Url, version: i32, text: String) {
        self.docs
            .insert(uri.clone(), DocumentSnapshot { version, text });
    }

    pub fn close(&self, uri: &Url) {
        self.docs.remove(uri);
    }

    pub fn get(&self, uri: &Url) -> Option<DocumentSnapshot> {
        self.docs.get(uri).map(|entry| entry.clone())
    }
}
