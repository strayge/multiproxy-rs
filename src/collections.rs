use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;

pub struct StorageSender {
    storage: Arc<Mutex<HashMap<u32, Sender<Vec<u8>>>>>,
}

impl StorageSender {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
        }
    }

    pub async fn insert(&self, key: u32, value: Sender<Vec<u8>>) {
        self.storage.lock().await.insert(key, value);
    }

    pub async fn remove(&self, key: u32) {
        self.storage.lock().await.remove(&key);
    }

    pub async fn contains_key(&self, key: u32) -> bool {
        self.storage.lock().await.contains_key(&key)
    }

    pub async fn get_randomized_key(&self, offset: u32) -> Option<u32> {
        let storage = self.storage.lock().await;
        let length = storage.len();
        if length == 0 {
            return None;
        }
        let index = offset % length as u32;
        let key = storage.keys().nth(index as usize).unwrap();
        Some(*key)
    }

    pub async fn send(&self, key: u32, data: Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(sender) = self.storage.lock().await.get(&key) {
            sender.send(data).await?;
        }
        Ok(())
    }
}

pub struct StorageId {
    storage: Arc<Mutex<HashMap<u32, u32>>>,
}

impl StorageId {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
        }
    }

    pub async fn insert(&self, key: u32, value: u32) {
        self.storage.lock().await.insert(key, value);
    }

    pub async fn remove(&self, key: u32) {
        self.storage.lock().await.remove(&key);
    }

    pub async fn contains_key(&self, key: u32) -> bool {
        self.storage.lock().await.contains_key(&key)
    }

    pub async fn get(&self, key: u32) -> Option<u32> {
        self.storage.lock().await.get(&key).cloned()
    }
}

pub struct StorageList {
    storage: Arc<Mutex<HashMap<u32, Vec<u32>>>>,
}

impl StorageList {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
        }
    }

    pub async fn insert(&self, key: u32, value: u32) {
        let mut storage = self.storage.lock().await;
        if !storage.contains_key(&key) {
            storage.insert(key, vec![]);
        }
        let list = storage.get_mut(&key).unwrap();
        list.push(value);
    }

    pub async fn remove(&self, key: u32) {
        self.storage.lock().await.remove(&key);
    }

    pub async fn contains_key(&self, key: u32) -> bool {
        self.storage.lock().await.contains_key(&key)
    }

    pub async fn get(&self, key: u32) -> Option<Vec<u32>> {
        self.storage.lock().await.get(&key).cloned()
    }

    pub async fn get_randomized(&self, key: u32, offset: u32) -> Option<u32> {
        if let Some(list) = self.storage.lock().await.get_mut(&key) {
            let length = list.len();
            if length == 0 {
                return None;
            }
            let index = (offset % length as u32) as usize;
            return Some(list[index]);
        } else {
            return None;
        }
    }
}

pub struct StorageSeqData {
    storage: Arc<Mutex<HashMap<u32, HashMap<u32, Vec<u8>>>>>,
}

impl StorageSeqData {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn clone(&self) -> Self {
        Self {
            storage: self.storage.clone(),
        }
    }

    pub async fn insert(&self, key: u32, seq: u32, value: Vec<u8>) {
        let mut map = self.storage.lock().await;
        if !map.contains_key(&key) {
            map.insert(key, HashMap::new());
        }
        map.get_mut(&key).unwrap().insert(seq, value);
    }

    pub async fn remove(&self, key: u32) {
        self.storage.lock().await.remove(&key);
    }

    pub async fn remove_seq(&self, key: u32, seq: u32) {
        if let Some(map) = self.storage.lock().await.get_mut(&key) {
            map.remove(&seq);
        }
    }

    pub async fn contains_key(&self, key: u32) -> bool {
        self.storage.lock().await.contains_key(&key)
    }

    pub async fn contains_seq(&self, key: u32, seq: u32) -> bool {
        if let Some(map) = self.storage.lock().await.get(&key) {
            map.contains_key(&seq)
        } else {
            false
        }
    }

    pub async fn get(&self, key: u32, seq: u32) -> Option<Vec<u8>> {
        if let Some(map) = self.storage.lock().await.get(&key) {
            map.get(&seq).cloned()
        } else {
            None
        }
    }
}
