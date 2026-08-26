use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::{ByteArray, LongArray, Value};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use rayon::prelude::*;

use zvcr::bench::discover::discover;
use zvcr::definitions::REGION_SIDELENGTH_SEGMENTS;
use zvcr::dimension::DimensionType;
use zvcr::io::file_location::RegionLocation;
use zvcr::io::serialize::experimental::ExperimentalReader;
use zvcr::io::serialize::types::Reader;
use zvcr::raw::SegmentData;
use zvcr::region::delta_sequence::DeltaSequence;
use zvcr::region::tile_entities::TileEntity;
use zvcr::time_utils::find_nearest_timestamp;
use zvcr::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS};

use crate::anvil::AnvilRegionWriter;
use crate::nbt;
use crate::packing;
use crate::registry::MinecraftRegistry;

const SECTION_SIZE_LIGHT: usize = 2048;

type SnapshotPair = (
    Vec<[u16; SECTION_SIZE_BLOCKS]>,
    Vec<[u16; SECTION_SIZE_BIOMES]>,
);

fn namespaced(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

fn region_subdir(dim: DimensionType) -> &'static str {
    match dim {
        DimensionType::Overworld => "region",
        DimensionType::Nether => "DIM-1/region",
        DimensionType::TheEnd => "DIM1/region",
    }
}

pub fn parse_dim(s: &str) -> Result<DimensionType, String> {
    let s = s.strip_prefix("minecraft:").unwrap_or(s);
    let dim = match s {
        "overworld" => DimensionType::Overworld,
        "nether" | "the_nether" => DimensionType::Nether,
        "end" | "the_end" => DimensionType::TheEnd,
        other => return Err(format!("unknown dimension: {other}")),
    };
    Ok(dim)
}

fn section_snapshots(seg: &SegmentData, epoch: i64) -> Option<SnapshotPair> {
    let block = (0..seg.block_sections.len())
        .map(|i| seg.block_sections[i].snapshot_before(epoch))
        .collect::<Option<Vec<_>>>()?;
    let biome = (0..seg.biome_sections.len())
        .map(|i| seg.biome_sections[i].snapshot_before(epoch))
        .collect::<Option<Vec<_>>>()?;
    Some((block, biome))
}

fn available_timestamps(seg: &SegmentData) -> Vec<i64> {
    let mut ts = Vec::new();
    for d in &seg.tile_entities.reverse_deltas {
        ts.push(d.timestamp);
    }
    for bs in &seg.block_sections {
        for s in bs.snapshots() {
            ts.push(s.timestamp);
        }
    }
    for bs in &seg.biome_sections {
        for s in bs.snapshots() {
            ts.push(s.timestamp);
        }
    }
    ts
}

fn tile_entity_value(
    te: &TileEntity,
    registry: &MinecraftRegistry,
    min_section_y: i32,
) -> Result<Value, String> {
    let mut te_map: HashMap<String, Value> = HashMap::new();
    let te_name = registry
        .tile_entity_name(te.tile_type)
        .unwrap_or(":unknown");
    te_map.insert("id".to_string(), Value::String(namespaced(te_name)));
    te_map.insert("keepPacked".to_string(), Value::Byte(0));
    te_map.insert("x".to_string(), Value::Int(te.pos.x as i32));
    te_map.insert(
        "y".to_string(),
        Value::Int(16 * min_section_y + te.pos.y as i32),
    );
    te_map.insert("z".to_string(), Value::Int(te.pos.z as i32));

    if !te.nbt.is_empty() {
        match nbt::payload_from_bytes(&te.nbt) {
            Ok(Value::Compound(parsed_map)) => {
                for (k, v) in parsed_map {
                    te_map.insert(k, v);
                }
            }
            Ok(_) => {}
            Err(e) => {
                return Err(format!(
                    "failed to parse tile entity nbt at ({},{},{}): {e}",
                    te.pos.x, te.pos.y, te.pos.z
                ));
            }
        }
    }
    Ok(Value::Compound(te_map))
}

fn section_value(
    index: usize,
    block_snap: &[u16; SECTION_SIZE_BLOCKS],
    biome_snap: &[u16; SECTION_SIZE_BIOMES],
    registry: &MinecraftRegistry,
    min_section_y: i32,
    light_template: &[i8],
) -> Value {
    let (block_palette, block_data) = packing::pack_section(block_snap, SECTION_SIZE_BLOCKS, 4);
    let (biome_palette, biome_data) = packing::pack_section(biome_snap, SECTION_SIZE_BIOMES, 0);

    let block_palette_list: Vec<Value> = block_palette
        .iter()
        .map(|&id| {
            let (name, props) = match registry.block_state(id) {
                Some(bs) => (bs.name.as_str(), Some(&bs.properties)),
                None => (":unknown", None),
            };
            let mut c = HashMap::new();
            c.insert("Name".to_string(), Value::String(namespaced(name)));
            if let Some(props) = props
                && !props.is_empty()
            {
                let mut pm = HashMap::new();
                for (k, v) in props {
                    pm.insert(k.clone(), Value::String(v.clone()));
                }
                c.insert("Properties".to_string(), Value::Compound(pm));
            }
            Value::Compound(c)
        })
        .collect();

    let biome_palette_list: Vec<Value> = biome_palette
        .iter()
        .map(|&id| {
            let name = registry.biome_name(id).unwrap_or(":unknown");
            Value::String(namespaced(name))
        })
        .collect();

    let mut block_states = HashMap::new();
    block_states.insert("palette".to_string(), Value::List(block_palette_list));
    if !block_data.is_empty() {
        block_states.insert(
            "data".to_string(),
            Value::LongArray(LongArray::new(block_data)),
        );
    }

    let mut biomes = HashMap::new();
    biomes.insert("palette".to_string(), Value::List(biome_palette_list));
    if !biome_data.is_empty() {
        biomes.insert(
            "data".to_string(),
            Value::LongArray(LongArray::new(biome_data)),
        );
    }

    let mut section = HashMap::new();
    section.insert(
        "Y".to_string(),
        Value::Byte((index as i32 + min_section_y) as i8),
    );
    section.insert("block_states".to_string(), Value::Compound(block_states));
    section.insert("biomes".to_string(), Value::Compound(biomes));
    section.insert(
        "BlockLight".to_string(),
        Value::ByteArray(ByteArray::new(light_template.to_vec())),
    );
    section.insert(
        "SkyLight".to_string(),
        Value::ByteArray(ByteArray::new(light_template.to_vec())),
    );
    Value::Compound(section)
}

fn chunk_value(
    abs_x: i32,
    abs_z: i32,
    min_section_y: i32,
    data_version: i32,
    te_compounds: Vec<Value>,
    section_compounds: Vec<Value>,
) -> Value {
    let mut root = HashMap::new();
    root.insert("DataVersion".to_string(), Value::Int(data_version));
    root.insert("Heightmaps".to_string(), Value::Compound(HashMap::new()));
    root.insert("InhabitedTime".to_string(), Value::Long(0));
    root.insert("LastUpdate".to_string(), Value::Long(0));
    root.insert("xPos".to_string(), Value::Int(abs_x));
    root.insert("zPos".to_string(), Value::Int(abs_z));
    root.insert("yPos".to_string(), Value::Int(min_section_y));
    root.insert(
        "Status".to_string(),
        Value::String("minecraft:full".to_string()),
    );
    root.insert("block_ticks".to_string(), Value::List(vec![]));
    root.insert("fluid_ticks".to_string(), Value::List(vec![]));
    root.insert("isLightOn".to_string(), Value::Byte(1));
    root.insert("block_entities".to_string(), Value::List(te_compounds));
    root.insert("sections".to_string(), Value::List(section_compounds));
    Value::Compound(root)
}

fn export_file(
    path: &Path,
    dim: DimensionType,
    reg765: &MinecraftRegistry,
    reg769: &MinecraftRegistry,
    out_dir: &Path,
    epoch: i64,
) -> Result<(), String> {
    let location = RegionLocation::from_file_name(dim, path)
        .ok_or_else(|| format!("unrecognized region filename: {}", path.display()))?;

    let region_data = ExperimentalReader::new()
        .read(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

    if !MinecraftRegistry::supports(region_data.protocol_version) {
        return Err(format!(
            "unsupported protocol version: {}",
            region_data.protocol_version
        ));
    }
    if region_data.dimension != dim {
        return Err(format!(
            "dimension mismatch in {}: file is {:?}, expected {:?}",
            path.display(),
            region_data.dimension,
            dim
        ));
    }

    let registry = match region_data.protocol_version {
        765 => reg765,
        769 => reg769,
        other => return Err(format!("unsupported protocol version: {other}")),
    };

    let rx = location.rx;
    let rz = location.rz;
    let min_section_y = dim.min_section_y();
    let light_template: Vec<i8> = vec![-1; SECTION_SIZE_LIGHT];

    let mut writer = AnvilRegionWriter::new();

    for x in 0..REGION_SIDELENGTH_SEGMENTS {
        for z in 0..REGION_SIDELENGTH_SEGMENTS {
            let idx = x * REGION_SIDELENGTH_SEGMENTS + z;
            let Some(seg) = &region_data.segments[idx] else {
                continue;
            };

            let section_count = seg.block_sections.len();
            if section_count == 0 {
                continue;
            }

            let (block_snaps, biome_snaps) = match section_snapshots(seg, epoch) {
                Some(s) => s,
                None => continue,
            };

            let timestamps = available_timestamps(seg);
            let mca_ts =
                find_nearest_timestamp(&timestamps, |t| *t, epoch).clamp(0, u32::MAX as i64) as u32;

            let mut tile_entities: Vec<TileEntity> = match seg.tile_entities.snapshot_before(epoch)
            {
                Some(map) => map.into_values().collect(),
                None => Vec::new(),
            };
            tile_entities.sort_by_key(|te| te.pos.packed());

            let mut te_compounds: Vec<Value> = Vec::new();
            for te in &tile_entities {
                te_compounds.push(tile_entity_value(te, registry, min_section_y)?);
            }

            let mut section_compounds = Vec::with_capacity(block_snaps.len());
            for i in 0..block_snaps.len() {
                section_compounds.push(section_value(
                    i,
                    &block_snaps[i],
                    &biome_snaps[i],
                    registry,
                    min_section_y,
                    &light_template,
                ));
            }

            let abs_x = rx * REGION_SIDELENGTH_SEGMENTS as i32 + x as i32;
            let abs_z = rz * REGION_SIDELENGTH_SEGMENTS as i32 + z as i32;

            let chunk = chunk_value(
                abs_x,
                abs_z,
                min_section_y,
                registry.data_version(),
                te_compounds,
                section_compounds,
            );

            let chunk_bytes = nbt::root_compound_to_bytes(&chunk)
                .map_err(|e| format!("failed to serialize chunk nbt in {}: {e}", path.display()))?;

            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&chunk_bytes).map_err(|e| e.to_string())?;
            let compressed = encoder.finish().map_err(|e| e.to_string())?;

            writer.write_chunk(x, z, mca_ts, &compressed).map_err(|e| {
                format!("failed to stage chunk ({x},{z}) in {}: {e}", path.display())
            })?;
        }
    }

    let bytes = writer
        .finish()
        .map_err(|e| format!("failed to assemble region {}: {e}", path.display()))?;

    let out_path = out_dir
        .join(region_subdir(dim))
        .join(format!("r.{rx}.{rz}.mca"));
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(&out_path, &bytes)
        .map_err(|e| format!("failed to write {}: {e}", out_path.display()))?;
    Ok(())
}

pub fn run_export(
    dim: DimensionType,
    in_dir: &Path,
    out_dir: &Path,
    registries_dir: &Path,
) -> ExitCode {
    let reg765 = match MinecraftRegistry::load(registries_dir, 765) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "failed to load registries for protocol 765 from {}: {e}",
                registries_dir.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let reg769 = match MinecraftRegistry::load(registries_dir, 769) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "failed to load registries for protocol 769 from {}: {e}",
                registries_dir.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let sources = discover(in_dir, None);
    if sources.is_empty() {
        eprintln!("no sources were found in {}", in_dir.display());
        return ExitCode::FAILURE;
    }

    let results: Vec<(PathBuf, Result<(), String>)> = sources
        .par_iter()
        .map(|path| {
            let res = export_file(path, dim, &reg765, &reg769, out_dir, epoch);
            (path.clone(), res)
        })
        .collect();

    let mut ok = 0usize;
    for (path, res) in &results {
        match res {
            Ok(()) => ok += 1,
            Err(e) => eprintln!("failed to export {}: {e}", path.display()),
        }
    }

    println!(
        "exported {ok}/{} region files to {}",
        results.len(),
        out_dir.display()
    );

    if ok == results.len() && ok > 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
