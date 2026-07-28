//! Encode/decode de embeddings (vetores `Vec<f32>`) em `Vec<u8>`
//! pra persistência na tabela `memory_embeddings.vec_blob`
//! (migration 0007).
//!
//! Formato: little-endian, 4 bytes por `f32`. Sem header, sem
//! checksum — o `dimensions` mora na coluna `dimensions` da
//! mesma linha (consistência verificada no read).
//!
//! `decode_embedding(blob, expected_dimensions)` valida que o
//! tamanho do blob é `expected_dimensions * 4` e devolve
//! `Err(MemoryError::EmbeddingDimensionMismatch)` se não for.
//! Isso evita ler bytes corrompidos como embedding válido.
//!
//! **Por que little-endian?** SQLite é portável e o `f32::to_le_bytes`
//! é estável — todo mundo lê na mesma ordem. Em plataformas
//! big-endian, `to_le_bytes` faz a conversão; em little-endian
//! (x86/ARM), é identidade. Ler com `f32::from_le_bytes` é
//! simétrico.

use crate::error::MemoryError;

/// Codifica um vetor de `f32` em bytes (little-endian, 4 bytes
/// por elemento). Usado pra persistir embeddings em
/// `memory_embeddings.vec_blob`.
///
/// ```text
/// encode_embedding(&[0.5, 1.0]) → 8 bytes (0.5_le ++ 1.0_le)
/// ```
#[must_use]
pub fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(embedding.len() * 4);
    for v in embedding {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Decodifica bytes em vetor de `f32`. Valida que o tamanho é
/// `expected_dimensions * 4` — erro de dimensão é estrutural
/// (consistência banco ↔ provider), não um bug do adapter.
pub fn decode_embedding(blob: &[u8], expected_dimensions: usize) -> Result<Vec<f32>, MemoryError> {
    let expected_bytes = expected_dimensions.checked_mul(4).ok_or_else(|| {
        MemoryError::EmbeddingDimensionMismatch {
            expected: expected_dimensions,
            actual: 0,
        }
    })?;
    if blob.len() != expected_bytes {
        return Err(MemoryError::EmbeddingDimensionMismatch {
            expected: expected_dimensions,
            actual: blob.len() / 4,
        });
    }
    let mut out = Vec::with_capacity(expected_dimensions);
    for chunk in blob.chunks_exact(4) {
        let bytes: [u8; 4] =
            chunk
                .try_into()
                .map_err(|_| MemoryError::EmbeddingDimensionMismatch {
                    expected: expected_dimensions,
                    actual: 0,
                })?;
        out.push(f32::from_le_bytes(bytes));
    }
    Ok(out)
}

/// Cosseno de similaridade entre dois vetores. Retorna 0.0 se
/// algum dos vetores tem norma zero (evita divisão por zero;
/// o caller trata como "sem similaridade" — `Retriever` cai
/// pra lexical+recência).
///
/// Resultado em [-1.0, 1.0], onde 1.0 = idênticos, 0.0 = ortogonais,
/// -1.0 = opostos. Em prática, embeddings de texto costumam
/// ficar em [0.0, 1.0] (a direção importa mais que o sinal).
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vetores devem ter mesma dimensão");
    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = f64::from(*x);
        let y = f64::from(*y);
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt())) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let original = vec![0.5_f32, -1.25, 0.0, std::f32::consts::PI];
        let encoded = encode_embedding(&original);
        assert_eq!(encoded.len(), original.len() * 4);

        let decoded = decode_embedding(&encoded, original.len()).expect("decode");
        assert_eq!(decoded.len(), original.len());
        for (a, b) in original.iter().zip(decoded.iter()) {
            assert!((a - b).abs() < 1e-6, "roundtrip divergiu: {a} vs {b}");
        }
    }

    #[test]
    fn encode_empty_returns_empty() {
        let encoded = encode_embedding(&[]);
        assert!(encoded.is_empty());
        let decoded = decode_embedding(&encoded, 0).expect("decode empty");
        assert!(decoded.is_empty());
    }

    #[test]
    fn decode_wrong_dimensions_rejected() {
        let encoded = encode_embedding(&[0.5, 1.0]);
        // Esperado 4 dimensões (16 bytes), só tem 8.
        assert!(matches!(
            decode_embedding(&encoded, 4),
            Err(MemoryError::EmbeddingDimensionMismatch { .. })
        ));
    }

    #[test]
    fn decode_truncated_rejected() {
        // 16 bytes esperados, só 14.
        let blob = vec![0u8; 14];
        assert!(matches!(
            decode_embedding(&blob, 4),
            Err(MemoryError::EmbeddingDimensionMismatch { .. })
        ));
    }

    #[test]
    fn cosine_identical_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_opposite_is_minus_one() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_zero_norm_returns_zero() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_similar_text_direction() {
        // Vetores com mesma direção mas magnitudes diferentes
        // → similaridade 1.0 (cosine ignora magnitude).
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![2.0, 4.0, 6.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }
}
