use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Live manager instance metadata used for rendezvous hashing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerInstance {
    pub id: String,
    pub capacity: u32,
    pub load: u32,
}

impl ManagerInstance {
    pub fn available(&self) -> bool {
        self.load < self.capacity
    }
}

/// Select a manager using weighted rendezvous hashing and capacity filtering.
/// Instances at or above capacity are skipped. Returns `None` if no candidates.
pub fn select_manager(key: &str, instances: &[ManagerInstance]) -> Option<ManagerInstance> {
    let mut best: Option<(f64, ManagerInstance)> = None;
    for inst in instances.iter().filter(|m| m.available()) {
        let hash = hash64(&(key, &inst.id));
        // Weight by available capacity (inverse of load). Add 1 to avoid div by zero.
        let weight = (inst.capacity.saturating_sub(inst.load).max(1)) as f64;
        let score = (hash as f64) * weight;
        if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, inst.clone()));
        }
    }
    best.map(|(_, inst)| inst)
}

fn hash64<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_only_available_instances() {
        let instances = vec![
            ManagerInstance {
                id: "full".into(),
                capacity: 1,
                load: 1,
            },
            ManagerInstance {
                id: "open".into(),
                capacity: 5,
                load: 0,
            },
        ];
        let selected = select_manager("host-1", &instances).expect("selected");
        assert_eq!(selected.id, "open");
    }

    #[test]
    fn deterministic_choice() {
        let instances = vec![
            ManagerInstance {
                id: "a".into(),
                capacity: 5,
                load: 1,
            },
            ManagerInstance {
                id: "b".into(),
                capacity: 5,
                load: 1,
            },
        ];
        let s1 = select_manager("host-123", &instances).unwrap();
        let s2 = select_manager("host-123", &instances).unwrap();
        assert_eq!(s1.id, s2.id);
    }

    #[test]
    fn prefers_more_available_capacity_on_ties() {
        let instances = vec![
            ManagerInstance {
                id: "heavy".into(),
                capacity: 10,
                load: 9,
            },
            ManagerInstance {
                id: "light".into(),
                capacity: 10,
                load: 1,
            },
        ];
        let selected = select_manager("host-xyz", &instances).unwrap();
        assert_eq!(selected.id, "light");
    }
}
