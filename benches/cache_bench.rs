//! SPDX-License-Identifier: MIT OR Apache-2.0

//! Benchmarks for file hashing performance.
//!
//! Compares sequential vs parallel file hashing for different file counts.
//!
//! Run with: cargo bench -p farm -- cache

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

// Re-implement the hashing logic inline for benchmarking
// (since hash_file_patterns is not public)
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;

fn hash_single_file(workspace: &Path, path: &Path) -> Result<(String, String, u64), String> {
    let rel_path_str = if let Ok(rel_path) = path.strip_prefix(workspace) {
        rel_path.to_string_lossy().to_string()
    } else {
        path.to_string_lossy().to_string()
    };

    let mut hasher = Sha256::new();
    let mut total_bytes: u64 = 0;

    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 8192];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buffer[..n]);
                total_bytes += n as u64;
            }
            Err(e) => return Err(e.to_string()),
        }
    }

    Ok((rel_path_str, hex::encode(hasher.finalize()), total_bytes))
}

fn hash_files_sequential(workspace: &Path, files: &[PathBuf]) -> String {
    let file_hashes: Vec<(String, String, u64)> = files
        .iter()
        .map(|path| hash_single_file(workspace, path).unwrap())
        .collect();

    let mut final_hasher = Sha256::new();
    for (rel_path, hash, _) in &file_hashes {
        final_hasher.update(rel_path.as_bytes());
        final_hasher.update(b"\0");
        final_hasher.update(hash.as_bytes());
        final_hasher.update(b"\0");
    }

    hex::encode(final_hasher.finalize())
}

fn hash_files_parallel(workspace: &Path, files: &[PathBuf]) -> String {
    let file_hashes: Vec<(String, String, u64)> = files
        .par_iter()
        .map(|path| hash_single_file(workspace, path).unwrap())
        .collect();

    let mut final_hasher = Sha256::new();
    for (rel_path, hash, _) in &file_hashes {
        final_hasher.update(rel_path.as_bytes());
        final_hasher.update(b"\0");
        final_hasher.update(hash.as_bytes());
        final_hasher.update(b"\0");
    }

    hex::encode(final_hasher.finalize())
}

/// Create test files of specified size
fn setup_test_files(temp: &TempDir, count: usize, size_bytes: usize) -> Vec<PathBuf> {
    let content: String = (0..size_bytes).map(|i| ((i % 26) as u8 + b'a') as char).collect();
    let mut files = Vec::with_capacity(count);

    for i in 0..count {
        let file_path = temp.path().join(format!("file_{:05}.txt", i));
        fs::write(&file_path, &content).unwrap();
        files.push(file_path);
    }

    files.sort();
    files
}

/// Benchmark file hashing with varying file counts
fn bench_file_hash_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("file_hash_scaling");
    
    // Test with small files (1KB each)
    let file_size = 1024;
    
    for count in [10, 50, 100, 500, 1000] {
        let temp = TempDir::new().unwrap();
        let files = setup_test_files(&temp, count, file_size);
        
        group.bench_with_input(
            BenchmarkId::new("sequential", count),
            &(&temp, &files),
            |b, (temp, files)| {
                b.iter(|| hash_files_sequential(black_box(temp.path()), black_box(files)))
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("parallel", count),
            &(&temp, &files),
            |b, (temp, files)| {
                b.iter(|| hash_files_parallel(black_box(temp.path()), black_box(files)))
            },
        );
    }
    
    group.finish();
}

/// Benchmark with varying file sizes (fixed count)
fn bench_file_size_impact(c: &mut Criterion) {
    let mut group = c.benchmark_group("file_size_impact");
    
    let count = 100;
    
    for size in [1024, 10 * 1024, 100 * 1024, 1024 * 1024] {
        let size_label = match size {
            1024 => "1KB",
            10240 => "10KB",
            102400 => "100KB",
            1048576 => "1MB",
            _ => "unknown",
        };
        
        let temp = TempDir::new().unwrap();
        let files = setup_test_files(&temp, count, size);
        
        group.bench_with_input(
            BenchmarkId::new("sequential", size_label),
            &(&temp, &files),
            |b, (temp, files)| {
                b.iter(|| hash_files_sequential(black_box(temp.path()), black_box(files)))
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("parallel", size_label),
            &(&temp, &files),
            |b, (temp, files)| {
                b.iter(|| hash_files_parallel(black_box(temp.path()), black_box(files)))
            },
        );
    }
    
    group.finish();
}

/// Quick sanity check that both methods produce identical hashes
fn bench_hash_correctness(c: &mut Criterion) {
    let temp = TempDir::new().unwrap();
    let files = setup_test_files(&temp, 50, 1024);
    
    c.bench_function("hash_correctness_check", |b| {
        b.iter(|| {
            let seq = hash_files_sequential(temp.path(), &files);
            let par = hash_files_parallel(temp.path(), &files);
            assert_eq!(seq, par);
            seq
        })
    });
}

criterion_group!(
    benches,
    bench_file_hash_scaling,
    bench_file_size_impact,
    bench_hash_correctness,
);

criterion_main!(benches);
