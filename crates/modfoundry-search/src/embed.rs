//! Text embeddings for search: the bge-small model and a test fake behind one trait.

use std::path::Path;

use candle_core::{Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use tokenizers::{Encoding, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

use crate::rank;

/// The instruction bge-small expects in front of a search query.
pub const QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// Turns texts into unit-length vectors of `dims()` values.
pub trait Embed {
    /// The length of every vector this embedder returns.
    fn dims(&self) -> usize;
    /// One unit-length vector per text, in order.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String>;
}

/// The text to embed for a search question.
pub fn query_text(question: &str) -> String {
    format!("{QUERY_PREFIX}{question}")
}

const BGE_DIMS: usize = 384;
const BATCH: usize = 32;
const MAX_TOKENS: usize = 512;

/// The bge-small model on the CPU.
pub struct BgeSmall {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl BgeSmall {
    /// Loads `config.json`, `tokenizer.json` and `model.safetensors` from `dir` on the CPU.
    pub fn load(dir: &Path) -> Result<Self, String> {
        Self::load_on(dir, Device::Cpu)
    }

    /// Loads the model from `dir` on the first CUDA device.
    pub fn load_gpu(dir: &Path) -> Result<Self, String> {
        let device = cuda_device()?;
        Self::load_on(dir, device)
    }

    fn load_on(dir: &Path, device: Device) -> Result<Self, String> {
        let config: Config = serde_json::from_slice(&read(&dir.join("config.json"))?)
            .map_err(|err| err.to_string())?;
        let mut tokenizer =
            Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|err| err.to_string())?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|err| err.to_string())?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        let weights = [dir.join("model.safetensors")];
        // SAFETY: the file is a read-only model download that nothing rewrites while loaded.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&weights, DTYPE, &device) }
            .map_err(|err| err.to_string())?;
        let model = BertModel::load(vb, &config).map_err(|err| err.to_string())?;
        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|err| err.to_string())?;
        let rows = encodings.len();
        let width = encodings.first().map_or(0, |e| e.get_ids().len());
        let ids = self.tensor(&encodings, Encoding::get_ids, rows, width)?;
        let type_ids = self.tensor(&encodings, Encoding::get_type_ids, rows, width)?;
        let mask = self.tensor(&encodings, Encoding::get_attention_mask, rows, width)?;
        let output = self
            .model
            .forward(&ids, &type_ids, Some(&mask))
            .map_err(|err| err.to_string())?;
        let first_tokens = output
            .i((.., 0))
            .and_then(|t| t.to_vec2::<f32>())
            .map_err(|err| err.to_string())?;
        Ok(first_tokens.into_iter().map(normalise).collect())
    }

    fn tensor(
        &self,
        encodings: &[Encoding],
        field: fn(&Encoding) -> &[u32],
        rows: usize,
        width: usize,
    ) -> Result<Tensor, String> {
        let data: Vec<u32> = encodings
            .iter()
            .flat_map(|e| field(e).iter().copied())
            .collect();
        Tensor::from_vec(data, (rows, width), &self.device).map_err(|err| err.to_string())
    }
}

fn cuda_device() -> Result<Device, String> {
    Device::new_cuda(0).map_err(|err| err.to_string())
}

/// Opens the first CUDA device and closes it again, so a failure names what a GPU run is missing.
pub fn check_gpu() -> Result<(), String> {
    cuda_device().map(drop)
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))
}

impl Embed for BgeSmall {
    fn dims(&self) -> usize {
        BGE_DIMS
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(BATCH) {
            vectors.extend(self.embed_batch(batch)?);
        }
        Ok(vectors)
    }
}

/// A deterministic embedder for tests: each keyword lands in one bucket.
pub struct FakeEmbed {
    pub dims: usize,
}

impl Embed for FakeEmbed {
    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts.iter().map(|text| self.vector(text)).collect())
    }
}

impl FakeEmbed {
    fn vector(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0.0; self.dims];
        for token in rank::tokens(text) {
            vector[bucket(&token, self.dims)] += 1.0;
        }
        if vector.iter().all(|value| *value == 0.0) {
            vector[0] = 1.0;
        }
        normalise(vector)
    }
}

fn bucket(token: &str, dims: usize) -> usize {
    let hash = token.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    (hash % dims as u64) as usize
}

fn normalise(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}
