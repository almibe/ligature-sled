// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//#![deny(missing_docs)]

mod query_tx;
mod write_tx;

use ligature::{
    Attribute, Dataset, Ligature, LigatureError, PersistedStatement, QueryFn, QueryTx, Range,
    Statement, WriteFn, WriteTx,
};
use ligature_kv::{
    chomp_assert, decode_dataset, encode_dataset, encode_dataset_match, prepend,
    ATTRIBUTE_ID_COUNTER_KEY, DATASET_PREFIX, ENTITY_ID_COUNTER_KEY, STRING_LITERAL_ID_COUNTER_KEY,
};
use query_tx::LigatureSledQueryTx;
use std::sync::RwLock;
use write_tx::LigatureSledWriteTx;

pub struct LigatureSled {
    //TODO eventually I won't need this but for now to support ReadTx range searches I need this lock
    //TODO an improvement on this would be pre-tree locks
    store_lock: RwLock<sled::Db>,
}

impl LigatureSled {
    /// Create/Open an instance of LigatureSled at the given path.
    pub fn new(path: String) -> Result<Self, sled::Error> {
        let instance = sled::open(path)?;
        Ok(Self {
            store_lock: RwLock::new(instance),
        })
    }

    /// Create a temporary instance of LigatureSled that is deleted on close.
    /// Pass Some(String) if you want it located at a given path or None if you want the default from Sled.
    pub fn temp(path: Option<String>) -> Result<Self, sled::Error> {
        match path {
            None => {
                let instance = sled::Config::default().temporary(true).open()?;
                Ok(Self {
                    store_lock: RwLock::new(instance),
                })
            }
            Some(p) => {
                let instance = sled::Config::default().temporary(true).path(p).open()?;
                Ok(Self {
                    store_lock: RwLock::new(instance),
                })
            }
        }
    }

    /// Create/Open an instance of LigatureSled with the given Sled config.
    /// Most people won't need this since the defaults are very good.
    pub fn from_config(config: sled::Config) -> Result<Self, sled::Error> {
        let instance = config.open()?;
        Ok(Self {
            store_lock: RwLock::new(instance),
        })
    }

    fn internal_dataset_exists(
        store: &sled::Db,
        encoded_dataset: &Vec<u8>,
    ) -> Result<bool, LigatureError> {
        store
            .contains_key(&encoded_dataset)
            .map_err(|_| LigatureError("Error checking for Dataset".to_string()))
    }
}

impl Ligature for LigatureSled {
    fn all_datasets(&self) -> Box<dyn Iterator<Item = Result<Dataset, LigatureError>>> {
        let store = self.store_lock.read().unwrap(); //to use map_err
        let iter = store.scan_prefix(vec![DATASET_PREFIX]); //store.iter();
        Box::new(iter.map(|ds| match ds {
            Ok(dataset) => decode_dataset(chomp_assert(DATASET_PREFIX, dataset.0.to_vec())?),
            Err(_) => Err(LigatureError("Error iterating Datasets.".to_string())),
        }))
    }

    fn dataset_exists(&self, dataset: &Dataset) -> Result<bool, LigatureError> {
        let store = self.store_lock.read().unwrap(); //to use map_err
        let encoded_dataset = prepend(DATASET_PREFIX, encode_dataset(&dataset));
        LigatureSled::internal_dataset_exists(&store, &encoded_dataset)
    }

    fn match_datasets_prefix(
        &self,
        prefix: &str,
    ) -> Box<dyn Iterator<Item = Result<Dataset, LigatureError>>> {
        let store_res = self.store_lock.read().map_err(|_| {
            LigatureError(
                "Error starting read transaction when matching dataset prefixes.".to_string(),
            )
        });
        match store_res {
            Ok(store) => {
                let encoded_prefix = prepend(DATASET_PREFIX, encode_dataset_match(prefix));
                let res = store.scan_prefix(encoded_prefix);
                Box::new(res.map(|value_res| match value_res {
                    Ok(value) => decode_dataset(chomp_assert(DATASET_PREFIX, value.0.to_vec())?),
                    Err(e) => Err(LigatureError(
                        "Error presfix matching Datasets.".to_string(),
                    )),
                }))
            }
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    }

    fn match_datasets_range(
        &self,
        from: &str,
        to: &str,
    ) -> Box<dyn Iterator<Item = Result<Dataset, LigatureError>>> {
        let store_res = self.store_lock.read().map_err(|_| {
            LigatureError(
                "Error starting read transaction when matching dataset ranges.".to_string(),
            )
        });
        match store_res {
            Ok(store) => {
                let encoded_from = prepend(DATASET_PREFIX, encode_dataset_match(from));
                let encoded_to = prepend(DATASET_PREFIX, encode_dataset_match(to));
                let res = store.range(encoded_from..encoded_to);
                Box::new(res.map(|value_res| match value_res {
                    Ok(value) => decode_dataset(chomp_assert(DATASET_PREFIX, value.0.to_vec())?),
                    Err(e) => Err(LigatureError(
                        "Error presfix matching Datasets.".to_string(),
                    )),
                }))
            }
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    }

    fn create_dataset(&self, dataset: &Dataset) -> Result<(), LigatureError> {
        let store = self.store_lock.write().map_err(|_| {
            LigatureError(format!(
                "Error starting write transaction when adding dataset {:?}.",
                dataset
            ))
        })?;
        let encoded_dataset = prepend(DATASET_PREFIX, encode_dataset(dataset));
        if !LigatureSled::internal_dataset_exists(&store, &encoded_dataset)? {
            store
                .insert(encoded_dataset, vec![])
                .map_err(|_| LigatureError(format!("Error inserting dataset {:?}.", dataset)))?;
            let dataset_tree = store.open_tree(dataset.name()).map_err(|_| {
                LigatureError(format!("Error creating dataset tree for {:?}.", dataset))
            })?;
            let id_start: u64 = 0;
            dataset_tree
                .insert(vec![ENTITY_ID_COUNTER_KEY], id_start.to_be_bytes().to_vec())
                .map_err(|_| {
                    LigatureError(format!(
                        "Error creating dataset entity id counter for {:?}.",
                        dataset
                    ))
                })?;
            dataset_tree
                .insert(
                    vec![ATTRIBUTE_ID_COUNTER_KEY],
                    id_start.to_be_bytes().to_vec(),
                )
                .map_err(|_| {
                    LigatureError(format!(
                        "Error creating dataset attribute id counter for {:?}.",
                        dataset
                    ))
                })?;
            dataset_tree
                .insert(
                    vec![STRING_LITERAL_ID_COUNTER_KEY],
                    id_start.to_be_bytes().to_vec(),
                )
                .map_err(|_| {
                    LigatureError(format!(
                        "Error creating dataset string literal id counter for {:?}.",
                        dataset
                    ))
                })?;
        }
        Ok(())
    }

    fn delete_dataset(&self, dataset: &Dataset) -> Result<(), LigatureError> {
        let store = self.store_lock.write().map_err(|_| {
            LigatureError("Error starting write transaction when deleting dataset.".to_string())
        })?;
        let encoded_dataset = prepend(DATASET_PREFIX, encode_dataset(dataset));
        if LigatureSled::internal_dataset_exists(&store, &encoded_dataset)? {
            store
                .remove(&encoded_dataset)
                .map_err(|_| LigatureError("Error removing dataset.".to_string()))?;
            store
                .drop_tree(dataset.name())
                .map_err(|_| LigatureError("Error dropping dataset tree.".to_string()))?;
        }
        Ok(())
    }

    fn query<T>(&self, dataset: &Dataset, f: QueryFn<T>) -> Result<T, LigatureError> {
        let store = self
            .store_lock
            .read()
            .map_err(|_| LigatureError("Error starting query transaction.".to_string()))?;
        let encoded_dataset = prepend(DATASET_PREFIX, encode_dataset(dataset));
        if LigatureSled::internal_dataset_exists(&store, &encoded_dataset)? {
            let tree = store
                .open_tree(dataset.name())
                .map_err(|_| LigatureError("Error starting query transaction.".to_string()))?;
            f(Box::new(&LigatureSledQueryTx::new(tree)))
        } else {
            Err(LigatureError(
                "Error starting query transaction.".to_string(),
            ))
        }
    }

    fn write<T>(&self, dataset: &Dataset, f: WriteFn<T>) -> Result<T, LigatureError> {
        let store = self
            .store_lock
            .write()
            .map_err(|_| LigatureError("Error starting write transaction.".to_string()))?;
        let encoded_dataset = prepend(DATASET_PREFIX, encode_dataset(dataset));
        if LigatureSled::internal_dataset_exists(&store, &encoded_dataset)? {
            let tree = store
                .open_tree(dataset.name())
                .map_err(|_| LigatureError("Error starting write transaction.".to_string()))?;
            let res = tree.transaction(|transaction_tree| {
                let write_tx = LigatureSledWriteTx::new(transaction_tree.clone());
                let res = f(Box::new(&write_tx));
                if write_tx.active.get() {
                    match res {
                        Ok(value) => Ok(value),
                        Err(err) => sled::transaction::abort(err),
                    }
                } else {
                    sled::transaction::abort(LigatureError("Aborting transaction.".to_string()))
                }
            });
            res.map_err(|e| LigatureError(format!("Error with writetx - {:?}.", e)))
        } else {
            Err(LigatureError(
                "Error starting write transaction.".to_string(),
            ))
        }
    }
}
