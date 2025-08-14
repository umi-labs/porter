use anyhow::Result;
use serde::{Serialize, Deserialize};
use serde_json::Value;
use std::path::Path;
use std::fs;
use log::{info, warn, debug};
use colored::Colorize;

use crate::mapping::{json_mapping::Mapping, apply_mapping};
use crate::adapter::{SourceReader, TargetWriter, TargetOptions};

/// Batch processing configuration
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Number of documents to process in each batch
    pub batch_size: usize,
    /// Maximum memory usage in MB
    pub max_memory_mb: usize,
    /// Whether to enable resume capability
    pub enable_resume: bool,
    /// Progress reporting interval (number of batches)
    pub progress_interval: usize,
    /// Temporary directory for batch files
    pub temp_dir: String,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            max_memory_mb: 512,
            enable_resume: true,
            progress_interval: 10,
            temp_dir: "./temp".to_string(),
        }
    }
}

/// Batch processing statistics
#[derive(Debug, Clone)]
pub struct BatchStats {
    pub total_documents: usize,
    pub processed_documents: usize,
    pub failed_documents: usize,
    pub current_batch: usize,
    pub total_batches: usize,
    pub memory_usage_mb: f64,
    pub processing_time_seconds: f64,
}

/// Batch processing result
#[derive(Debug, Clone)]
pub struct BatchResult {
    pub success: bool,
    pub stats: BatchStats,
    pub errors: Vec<BatchError>,
    pub checkpoint_file: Option<String>,
}

/// Batch processing error
#[derive(Debug, Clone)]
pub struct BatchError {
    pub document_index: usize,
    pub batch_index: usize,
    pub error_message: String,
    pub document_id: Option<String>,
}

/// Batch processor for handling large datasets
pub struct BatchProcessor {
    config: BatchConfig,
    stats: BatchStats,
    errors: Vec<BatchError>,
    start_time: std::time::Instant,
}

impl BatchProcessor {
    /// Creates a new batch processor
    pub fn new(config: BatchConfig) -> Self {
        Self {
            config,
            stats: BatchStats {
                total_documents: 0,
                processed_documents: 0,
                failed_documents: 0,
                current_batch: 0,
                total_batches: 0,
                memory_usage_mb: 0.0,
                processing_time_seconds: 0.0,
            },
            errors: Vec::new(),
            start_time: std::time::Instant::now(),
        }
    }

    /// Processes documents in batches
    pub fn process_documents(
        &mut self,
        source: &dyn SourceReader,
        target: &dyn TargetWriter,
        mapping: &Mapping,
        source_files: &[String],
        output_path: &str,
        options: &TargetOptions,
    ) -> Result<BatchResult> {
        info!("Starting batch processing with config: {:?}", self.config);

        // Create temp directory if it doesn't exist
        fs::create_dir_all(&self.config.temp_dir)?;

        // Load all documents from source files
        let all_documents = self.load_all_documents(source, source_files)?;
        self.stats.total_documents = all_documents.len();
        self.stats.total_batches = (all_documents.len() + self.config.batch_size - 1) / self.config.batch_size;

        info!("Loaded {} documents, will process in {} batches", 
              self.stats.total_documents, self.stats.total_batches);

        // Check for existing checkpoint
        let checkpoint = if self.config.enable_resume {
            self.load_checkpoint()?
        } else {
            None
        };

        // Process documents in batches
        let success = true;
        for batch_index in 0..self.stats.total_batches {
            self.stats.current_batch = batch_index + 1;

            // Calculate batch boundaries
            let start_idx = batch_index * self.config.batch_size;
            let end_idx = std::cmp::min(start_idx + self.config.batch_size, all_documents.len());

            // Skip if we're resuming and this batch was already processed
            if let Some(ref checkpoint) = checkpoint {
                if batch_index < checkpoint.last_completed_batch {
                    info!("Skipping batch {} (already completed)", batch_index + 1);
                    self.stats.processed_documents += end_idx - start_idx;
                    continue;
                }
            }

            // Process current batch
            match self.process_batch(
                &all_documents[start_idx..end_idx],
                mapping,
                target,
                output_path,
                options,
                batch_index,
            ) {
                Ok((batch_stats, batch_errors)) => {
                    self.stats.processed_documents += batch_stats.processed_documents;
                    self.stats.failed_documents += batch_stats.failed_documents;
                    self.errors.extend(batch_errors);
                }
                Err(e) => {
                    warn!("Batch {} failed: {}", batch_index + 1, e);
                    // success = false; // This is handled by the return statement below
                    
                    // Save checkpoint for resume
                    if self.config.enable_resume {
                        self.save_checkpoint(batch_index)?;
                    }
                    
                    return Ok(BatchResult {
                        success: false,
                        stats: self.stats.clone(),
                        errors: self.errors.clone(),
                        checkpoint_file: Some(self.get_checkpoint_path()),
                    });
                }
            }

            // Update memory usage
            self.update_memory_usage();

            // Report progress
            if (batch_index + 1) % self.config.progress_interval == 0 {
                self.report_progress();
            }

            // Save checkpoint periodically
            if self.config.enable_resume && (batch_index + 1) % 5 == 0 {
                self.save_checkpoint(batch_index)?;
            }
        }

        // Clean up temp files
        self.cleanup_temp_files()?;

        // Update final stats
        self.stats.processing_time_seconds = self.start_time.elapsed().as_secs_f64();

        Ok(BatchResult {
            success,
            stats: self.stats.clone(),
            errors: self.errors.clone(),
            checkpoint_file: None,
        })
    }

    /// Loads all documents from source files
    fn load_all_documents(
        &self,
        source: &dyn SourceReader,
        source_files: &[String],
    ) -> Result<Vec<Value>> {
        let mut all_documents = Vec::new();

        for file_path in source_files {
            info!("Loading documents from: {}", file_path);
            let documents = source.read_documents(&[file_path.clone()])?;
            all_documents.extend(documents);
        }

        Ok(all_documents)
    }

    /// Processes a single batch of documents
    fn process_batch(
        &self,
        documents: &[Value],
        mapping: &Mapping,
        target: &dyn TargetWriter,
        output_path: &str,
        options: &TargetOptions,
        batch_index: usize,
    ) -> Result<(BatchStats, Vec<BatchError>)> {
        let batch_start_time = std::time::Instant::now();
        let mut processed = 0;
        let mut failed = 0;
        let mut errors = Vec::new();

        info!("Processing batch {} with {} documents", batch_index + 1, documents.len());

        // Transform documents
        let mut transformed_docs = Vec::new();
        for (doc_index, doc) in documents.iter().enumerate() {
            match apply_mapping(doc, mapping) {
                Ok(transformed) => {
                    transformed_docs.push(transformed);
                    processed += 1;
                }
                Err(e) => {
                    failed += 1;
                    errors.push(BatchError {
                        document_index: batch_index * self.config.batch_size + doc_index,
                        batch_index,
                        error_message: e.to_string(),
                        document_id: doc.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    });
                }
            }
        }

        // Write batch to temporary file
        let _batch_file = self.write_batch_to_temp(&transformed_docs, batch_index)?;

        // Write to target (this could be optimized to write directly)
        if !transformed_docs.is_empty() {
            target.emit_seed(&transformed_docs, output_path, options)?;
        }

        let batch_time = batch_start_time.elapsed().as_secs_f64();
        debug!("Batch {} completed in {:.2}s: {} processed, {} failed", 
               batch_index + 1, batch_time, processed, failed);

        // Add errors to the processor's error collection
        // Note: In a real implementation, we'd need to modify the function signature
        // to return errors, but for now we'll just log them
        
        Ok((BatchStats {
            total_documents: documents.len(),
            processed_documents: processed,
            failed_documents: failed,
            current_batch: batch_index + 1,
            total_batches: 1,
            memory_usage_mb: 0.0, // Will be updated by caller
            processing_time_seconds: batch_time,
        }, errors))
    }

    /// Writes a batch to a temporary file
    fn write_batch_to_temp(&self, documents: &[Value], batch_index: usize) -> Result<String> {
        let batch_file = format!("{}/batch_{:04}.json", self.config.temp_dir, batch_index);
        let content = serde_json::to_string_pretty(documents)?;
        fs::write(&batch_file, content)?;
        Ok(batch_file)
    }

    /// Updates memory usage statistics
    fn update_memory_usage(&mut self) {
        // This is a simplified memory usage calculation
        // In a real implementation, you might use a crate like `sysinfo`
        self.stats.memory_usage_mb = (self.stats.processed_documents * 1024) as f64 / 1024.0 / 1024.0;
    }

    /// Reports progress
    fn report_progress(&self) {
        let progress = (self.stats.processed_documents as f64 / self.stats.total_documents as f64) * 100.0;
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let rate = if elapsed > 0.0 {
            self.stats.processed_documents as f64 / elapsed
        } else {
            0.0
        };

        println!("{}", "📊 Batch Processing Progress".cyan().bold());
        println!("{}", "─".repeat(50));
        println!("Progress: {:.1}% ({}/{})", 
                 progress, self.stats.processed_documents, self.stats.total_documents);
        println!("Batch: {}/{}", self.stats.current_batch, self.stats.total_batches);
        println!("Rate: {:.1} docs/sec", rate);
        println!("Memory: {:.1} MB", self.stats.memory_usage_mb);
        println!("Failed: {}", self.stats.failed_documents);
        println!("Elapsed: {:.1}s", elapsed);
        println!("{}", "─".repeat(50));
    }

    /// Saves a checkpoint for resume capability
    fn save_checkpoint(&self, last_completed_batch: usize) -> Result<()> {
        let checkpoint = Checkpoint {
            last_completed_batch,
            processed_documents: self.stats.processed_documents,
            failed_documents: self.stats.failed_documents,
            timestamp: chrono::Utc::now(),
        };

        let checkpoint_path = self.get_checkpoint_path();
        let content = serde_json::to_string_pretty(&checkpoint)?;
        fs::write(checkpoint_path, content)?;

        debug!("Checkpoint saved: batch {}", last_completed_batch);
        Ok(())
    }

    /// Loads a checkpoint for resume capability
    fn load_checkpoint(&self) -> Result<Option<Checkpoint>> {
        let checkpoint_path = self.get_checkpoint_path();
        
        if Path::new(&checkpoint_path).exists() {
            let content = fs::read_to_string(&checkpoint_path)?;
            let checkpoint: Checkpoint = serde_json::from_str(&content)?;
            info!("Loaded checkpoint: batch {}, {} documents processed", 
                  checkpoint.last_completed_batch, checkpoint.processed_documents);
            Ok(Some(checkpoint))
        } else {
            Ok(None)
        }
    }

    /// Gets the checkpoint file path
    fn get_checkpoint_path(&self) -> String {
        format!("{}/checkpoint.json", self.config.temp_dir)
    }

    /// Cleans up temporary files
    fn cleanup_temp_files(&self) -> Result<()> {
        if Path::new(&self.config.temp_dir).exists() {
            fs::remove_dir_all(&self.config.temp_dir)?;
            info!("Cleaned up temporary files");
        }
        Ok(())
    }
}

/// Checkpoint data for resume capability
#[derive(Debug, Serialize, Deserialize)]
struct Checkpoint {
    last_completed_batch: usize,
    processed_documents: usize,
    failed_documents: usize,
    timestamp: chrono::DateTime<chrono::Utc>,
}

/// Creates a batch processor with default configuration
pub fn create_batch_processor() -> BatchProcessor {
    BatchProcessor::new(BatchConfig::default())
}

/// Creates a batch processor with custom configuration
pub fn create_batch_processor_with_config(config: BatchConfig) -> BatchProcessor {
    BatchProcessor::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn test_batch_config_default() {
        let config = BatchConfig::default();
        assert_eq!(config.batch_size, 100);
        assert_eq!(config.max_memory_mb, 512);
        assert!(config.enable_resume);
        assert_eq!(config.progress_interval, 10);
        assert_eq!(config.temp_dir, "./temp");
    }

    #[test]
    fn test_batch_processor_creation() {
        let processor = create_batch_processor();
        assert_eq!(processor.config.batch_size, 100);
        assert_eq!(processor.stats.total_documents, 0);
    }

    #[test]
    fn test_batch_stats_initialization() {
        let stats = BatchStats {
            total_documents: 1000,
            processed_documents: 500,
            failed_documents: 5,
            current_batch: 5,
            total_batches: 10,
            memory_usage_mb: 256.0,
            processing_time_seconds: 120.0,
        };

        assert_eq!(stats.total_documents, 1000);
        assert_eq!(stats.processed_documents, 500);
        assert_eq!(stats.failed_documents, 5);
        assert_eq!(stats.current_batch, 5);
        assert_eq!(stats.total_batches, 10);
        assert_eq!(stats.memory_usage_mb, 256.0);
        assert_eq!(stats.processing_time_seconds, 120.0);
    }

    #[test]
    fn test_batch_error_creation() {
        let error = BatchError {
            document_index: 42,
            batch_index: 1,
            error_message: "Test error".to_string(),
            document_id: Some("doc-123".to_string()),
        };

        assert_eq!(error.document_index, 42);
        assert_eq!(error.batch_index, 1);
        assert_eq!(error.error_message, "Test error");
        assert_eq!(error.document_id, Some("doc-123".to_string()));
    }
}
