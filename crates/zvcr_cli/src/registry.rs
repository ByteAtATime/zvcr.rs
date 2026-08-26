use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

pub struct BlockState {
    pub name: String,
    pub properties: HashMap<String, String>,
}

fn missing_block_state() -> &'static BlockState {
    static INSTANCE: OnceLock<BlockState> = OnceLock::new();
    INSTANCE.get_or_init(|| BlockState {
        name: ":unknown".to_string(),
        properties: HashMap::new(),
    })
}

pub struct MinecraftRegistry {
    protocol: u16,
    blocks: HashMap<u16, BlockState>,
    biomes: HashMap<u16, String>,
    tile_entities: HashMap<u32, String>,
}

fn parse_block_state_name(name: &str) -> (String, HashMap<String, String>) {
    let open = name.find('[');
    let close = name.find(']');
    let base = match open {
        None => name.to_string(),
        Some(o) => name[..o].to_string(),
    };
    let props_str = match (open, close) {
        (Some(o), Some(c)) if c > o => &name[o + 1..c],
        _ => "",
    };
    let mut properties = HashMap::new();
    if !props_str.is_empty() && props_str != "-" {
        for token in props_str.split(',') {
            if token.is_empty() {
                continue;
            }
            if let Some(eq) = token.find('=') {
                let key = token[..eq].to_string();
                let value = token[eq + 1..].to_string();
                properties.insert(key, value);
            }
        }
    }
    (base, properties)
}

fn load_entries(file: &Path) -> Result<Vec<serde_json::Value>, String> {
    let content = std::fs::read_to_string(file)
        .map_err(|e| format!("failed to read {}: {e}", file.display()))?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    json.get("entries")
        .and_then(|e| e.as_array())
        .cloned()
        .ok_or_else(|| format!("missing entries array in {}", file.display()))
}

impl MinecraftRegistry {
    pub fn supports(protocol: u16) -> bool {
        matches!(protocol, 765 | 769)
    }

    pub fn load(root: &Path, protocol: u16) -> Result<MinecraftRegistry, String> {
        if !Self::supports(protocol) {
            return Err(format!("unsupported protocol {protocol}"));
        }
        let dir = root.join(protocol.to_string());
        let block_entries = load_entries(&dir.join("blockstates.json"))?;
        let biome_entries = load_entries(&dir.join("biome_properties.json"))?;
        let tile_entries = load_entries(&dir.join("tile_entities.json"))?;

        let mut blocks = HashMap::new();
        for entry in block_entries {
            let id = entry
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "block entry missing id".to_string())? as u16;
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "block entry missing name".to_string())?
                .to_string();
            let (base, properties) = parse_block_state_name(&name);
            blocks.insert(
                id,
                BlockState {
                    name: base,
                    properties,
                },
            );
        }

        let mut biomes = HashMap::new();
        for entry in biome_entries {
            let id = entry
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "biome entry missing id".to_string())? as u16;
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "biome entry missing name".to_string())?
                .to_string();
            biomes.insert(id, name);
        }

        let mut tile_entities = HashMap::new();
        for entry in tile_entries {
            let id = entry
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| "tile entity entry missing id".to_string())?
                as u32;
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "tile entity entry missing name".to_string())?
                .to_string();
            tile_entities.insert(id, name);
        }

        Ok(MinecraftRegistry {
            protocol,
            blocks,
            biomes,
            tile_entities,
        })
    }

    pub fn block_state(&self, id: u16) -> Option<&BlockState> {
        if id == 0xFFFF {
            return Some(missing_block_state());
        }
        self.blocks.get(&id)
    }

    pub fn biome_name(&self, id: u16) -> Option<&str> {
        self.biomes.get(&id).map(|s| s.as_str())
    }

    pub fn tile_entity_name(&self, id: u32) -> Option<&str> {
        self.tile_entities.get(&id).map(|s| s.as_str())
    }

    pub fn data_version(&self) -> i32 {
        match self.protocol {
            765 => 3700,
            769 => 4189,
            _ => 0,
        }
    }
}
