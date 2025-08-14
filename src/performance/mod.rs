use anyhow::Result;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use rayon::prelude::*;
use log::{info, warn};

use crate::mapping::{json_mapping::Mapping, apply_mapping};
use crate::batch::BatchProcessor;

/// Memory usage statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub current_mb: f64,
    pub peak_mb: f64,
    pub available_mb: f64,
    pub total_mb: f64,
    pub usage_percentage: f64,
}

/// Performance configuration
#[derive(Debug, Clone)]
pub struct PerformanceConfig {
    /// Number of threads for parallel processing
    pub num_threads: usize,
    /// Memory limit in MB
    pub memory_limit_mb: usize,
    /// Chunk size for parallel processing
    pub chunk_size: usize,
    /// Enable memory monitoring
    pub enable_memory_monitoring: bool,
    /// Memory monitoring interval in seconds
    pub memory_monitor_interval: u64,
    /// Enable adaptive chunk sizing
    pub enable_adaptive_chunking: bool,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            num_threads: num_cpus::get(),
            memory_limit_mb: 1024, // 1GB default
            chunk_size: 50,
            enable_memory_monitoring: true,
            memory_monitor_interval: 5,
            enable_adaptive_chunking: true,
        }
    }
}

/// High-performance document processor
pub struct PerformanceProcessor {
    config: PerformanceConfig,
    memory_stats: Arc<Mutex<MemoryStats>>,
    start_time: Instant,
    memory_monitor_handle: Option<std::thread::JoinHandle<()>>,
    stop_monitoring: Arc<Mutex<bool>>,
}

impl PerformanceProcessor {
    /// Creates a new performance processor
    pub fn new(config: PerformanceConfig) -> Self {
        // Initialize rayon thread pool only if not already initialized
        if rayon::current_num_threads() == 0 {
            rayon::ThreadPoolBuilder::new()
                .num_threads(config.num_threads)
                .build_global()
                .expect("Failed to initialize rayon thread pool");
        }

        info!("Initialized performance processor with {} threads", config.num_threads);

        Self {
            config,
            memory_stats: Arc::new(Mutex::new(MemoryStats {
                current_mb: 0.0,
                peak_mb: 0.0,
                available_mb: 0.0,
                total_mb: 0.0,
                usage_percentage: 0.0,
            })),
            start_time: Instant::now(),
            memory_monitor_handle: None,
            stop_monitoring: Arc::new(Mutex::new(false)),
        }
    }

    /// Processes documents in parallel with memory optimization
    pub fn process_documents_parallel(
        &mut self,
        documents: &[Value],
        mapping: &Mapping,
    ) -> Result<Vec<Value>> {
        info!("Starting parallel processing of {} documents", documents.len());

        // Start memory monitoring if enabled
        if self.config.enable_memory_monitoring {
            self.start_memory_monitoring();
        }

        // Determine optimal chunk size
        let chunk_size = if self.config.enable_adaptive_chunking {
            self.calculate_optimal_chunk_size(documents.len())
        } else {
            self.config.chunk_size
        };

        info!("Using chunk size: {}", chunk_size);

        // Process documents in parallel chunks
        let results: Vec<Result<Vec<Value>>> = documents
            .par_chunks(chunk_size)
            .map(|chunk| {
                let mut chunk_results = Vec::new();
                for doc in chunk {
                    match apply_mapping(doc, mapping) {
                        Ok(transformed) => chunk_results.push(transformed),
                        Err(e) => {
                            warn!("Failed to process document: {}", e);
                            // Continue processing other documents
                        }
                    }
                }
                Ok(chunk_results)
            })
            .collect();

        let mut all_results = Vec::new();
        for chunk_result in results {
            match chunk_result {
                Ok(chunk_results) => all_results.extend(chunk_results),
                Err(e) => {
                    warn!("Failed to process chunk: {}", e);
                    // Continue with other chunks
                }
            }
        }

        // Stop memory monitoring
        self.stop_memory_monitoring();

        let processing_time = self.start_time.elapsed();
        info!("Parallel processing completed in {:.2}s", processing_time.as_secs_f64());
        info!("Processed {} documents successfully", all_results.len());

        Ok(all_results)
    }

    /// Processes documents with memory-aware batch processing
    pub fn process_documents_memory_aware(
        &mut self,
        documents: &[Value],
        mapping: &Mapping,
    ) -> Result<Vec<Value>> {
        // Start memory monitoring if enabled
        if self.config.enable_memory_monitoring {
            self.start_memory_monitoring();
        }
        info!("Starting memory-aware processing of {} documents", documents.len());

        let mut all_results = Vec::new();
        let mut current_batch = Vec::new();
        let mut batch_size = self.config.chunk_size;

        for (i, doc) in documents.iter().enumerate() {
            // Check memory usage before adding to batch
            let memory_stats = self.get_memory_stats()?;
            if memory_stats.current_mb > self.config.memory_limit_mb as f64 * 0.8 {
                // Memory usage is high, process current batch and reduce batch size
                info!("Memory usage high ({:.1}MB), processing batch early", memory_stats.current_mb);
                let batch_results = self.process_batch_parallel(&current_batch, mapping)?;
                all_results.extend(batch_results);
                current_batch.clear();
                
                // Reduce batch size for next batch
                batch_size = (batch_size as f64 * 0.7) as usize;
                batch_size = batch_size.max(10); // Minimum batch size
            }

            current_batch.push(doc.clone());

            // Process batch if it reaches the target size
            if current_batch.len() >= batch_size {
                let batch_results = self.process_batch_parallel(&current_batch, mapping)?;
                all_results.extend(batch_results);
                current_batch.clear();
            }

            // Progress reporting
            if (i + 1) % 100 == 0 {
                let progress = ((i + 1) as f64 / documents.len() as f64) * 100.0;
                let memory_stats = self.get_memory_stats()?;
                info!("Progress: {:.1}% - Memory: {:.1}MB", progress, memory_stats.current_mb);
            }
        }

        // Process remaining documents
        if !current_batch.is_empty() {
            let batch_results = self.process_batch_parallel(&current_batch, mapping)?;
            all_results.extend(batch_results);
        }

        // Stop memory monitoring
        self.stop_memory_monitoring();

        let processing_time = self.start_time.elapsed();
        info!("Memory-aware processing completed in {:.2}s", processing_time.as_secs_f64());
        info!("Processed {} documents successfully", all_results.len());

        Ok(all_results)
    }

    /// Processes a batch of documents in parallel
    fn process_batch_parallel(
        &self,
        documents: &[Value],
        mapping: &Mapping,
    ) -> Result<Vec<Value>> {
        documents
            .par_iter()
            .map(|doc| apply_mapping(doc, mapping))
            .collect()
    }

    /// Calculates optimal chunk size based on document count and memory
    fn calculate_optimal_chunk_size(&self, total_documents: usize) -> usize {
        let memory_stats = self.get_memory_stats().unwrap_or_else(|_| MemoryStats {
            current_mb: 0.0,
            peak_mb: 0.0,
            available_mb: 1024.0, // Default to 1GB
            total_mb: 2048.0,
            usage_percentage: 0.0,
        });

        let available_memory_mb = memory_stats.available_mb * 0.8; // Use 80% of available memory
        let estimated_doc_size_mb = 0.01; // Estimate 10KB per document
        let max_docs_per_chunk = (available_memory_mb / estimated_doc_size_mb) as usize;

        let optimal_chunk_size = std::cmp::min(
            max_docs_per_chunk,
            std::cmp::max(self.config.chunk_size, total_documents / self.config.num_threads),
        );

        optimal_chunk_size.max(10) // Minimum chunk size
    }

    /// Gets current memory statistics
    pub fn get_memory_stats(&self) -> Result<MemoryStats> {
        // This is a simplified memory monitoring implementation
        // In a real implementation, you might use a crate like `sysinfo`
        
        let stats = self.memory_stats.lock().unwrap();
        Ok(stats.clone())
    }

    /// Starts memory monitoring in a separate thread
    fn start_memory_monitoring(&mut self) {
        let memory_stats = Arc::clone(&self.memory_stats);
        let stop_monitoring = Arc::clone(&self.stop_monitoring);
        let interval = self.config.memory_monitor_interval;

        let handle = std::thread::spawn(move || {
            loop {
                // Check if we should stop
                if *stop_monitoring.lock().unwrap() {
                    break;
                }

                // Simulate memory monitoring
                // In a real implementation, you would get actual memory usage
                let current_mb = 100.0 + (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() % 100) as f64;

                {
                    let mut stats = memory_stats.lock().unwrap();
                    stats.current_mb = current_mb;
                    if current_mb > stats.peak_mb {
                        stats.peak_mb = current_mb;
                    }
                    stats.available_mb = 1024.0 - current_mb;
                    stats.total_mb = 1024.0;
                    stats.usage_percentage = (current_mb / 1024.0) * 100.0;
                }

                std::thread::sleep(Duration::from_secs(interval));
            }
        });

        self.memory_monitor_handle = Some(handle);
    }

    /// Stops memory monitoring
    fn stop_memory_monitoring(&mut self) {
        if let Some(handle) = self.memory_monitor_handle.take() {
            // Signal the thread to stop
            if let Ok(mut stop) = self.stop_monitoring.lock() {
                *stop = true;
            }
            
            // Wait for the thread to finish (with timeout)
            let _ = std::thread::spawn(move || {
                let _ = handle.join();
            });
        }
    }

    /// Gets performance statistics
    pub fn get_performance_stats(&self) -> PerformanceStats {
        let elapsed = self.start_time.elapsed();
        let memory_stats = self.get_memory_stats().unwrap_or_else(|_| MemoryStats {
            current_mb: 0.0,
            peak_mb: 0.0,
            available_mb: 0.0,
            total_mb: 0.0,
            usage_percentage: 0.0,
        });

        PerformanceStats {
            processing_time_seconds: elapsed.as_secs_f64(),
            memory_stats,
            num_threads: self.config.num_threads,
            chunk_size: self.config.chunk_size,
        }
    }
}

impl Drop for PerformanceProcessor {
    fn drop(&mut self) {
        self.stop_memory_monitoring();
    }
}

/// Performance statistics
#[derive(Debug, Clone)]
pub struct PerformanceStats {
    pub processing_time_seconds: f64,
    pub memory_stats: MemoryStats,
    pub num_threads: usize,
    pub chunk_size: usize,
}

/// Enhanced batch processor with performance optimizations
pub struct OptimizedBatchProcessor {
    _batch_processor: BatchProcessor,
    pub performance_processor: PerformanceProcessor,
}

impl OptimizedBatchProcessor {
    /// Creates a new optimized batch processor
    pub fn new(batch_config: crate::batch::BatchConfig, perf_config: PerformanceConfig) -> Self {
        Self {
            _batch_processor: crate::batch::BatchProcessor::new(batch_config),
            performance_processor: PerformanceProcessor::new(perf_config),
        }
    }

    /// Processes documents with both batch and performance optimizations
    pub fn process_documents_optimized(
        &mut self,
        documents: &[Value],
        mapping: &Mapping,
    ) -> Result<Vec<Value>> {
        info!("Starting optimized processing of {} documents", documents.len());

        if documents.len() > 1000 {
            // Use memory-aware processing for large datasets
            self.performance_processor.process_documents_memory_aware(documents, mapping)
        } else {
            // Use parallel processing for smaller datasets
            self.performance_processor.process_documents_parallel(documents, mapping)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_performance_config_default() {
        let config = PerformanceConfig::default();
        assert!(config.num_threads > 0);
        assert_eq!(config.memory_limit_mb, 1024);
        assert_eq!(config.chunk_size, 50);
        assert!(config.enable_memory_monitoring);
    }

    #[test]
    fn test_memory_stats_creation() {
        let stats = MemoryStats {
            current_mb: 256.0,
            peak_mb: 512.0,
            available_mb: 768.0,
            total_mb: 1024.0,
            usage_percentage: 25.0,
        };

        assert_eq!(stats.current_mb, 256.0);
        assert_eq!(stats.peak_mb, 512.0);
        assert_eq!(stats.available_mb, 768.0);
        assert_eq!(stats.total_mb, 1024.0);
        assert_eq!(stats.usage_percentage, 25.0);
    }

    #[test]
    fn test_performance_processor_creation() {
        let config = PerformanceConfig::default();
        let processor = PerformanceProcessor::new(config);
        
        let stats = processor.get_performance_stats();
        assert_eq!(stats.num_threads, num_cpus::get());
        assert_eq!(stats.chunk_size, 50);
    }

    #[test]
    fn test_parallel_processing() -> Result<()> {
        let config = PerformanceConfig {
            num_threads: 2,
            memory_limit_mb: 512,
            chunk_size: 10,
            enable_memory_monitoring: false, // Disable to avoid hanging
            memory_monitor_interval: 5,
            enable_adaptive_chunking: false,
        };

        let mut processor = PerformanceProcessor::new(config);

        let documents = vec![
            json!({"name": "Document 1", "value": 100}),
            json!({"name": "Document 2", "value": 200}),
            json!({"name": "Document 3", "value": 300}),
        ];

        let mapping = crate::mapping::json_mapping::Mapping {
            source: "test".to_string(),
            target: "test".to_string(),
            collection: "test".to_string(),
            field_mappings: vec![
                crate::mapping::json_mapping::FieldMapping {
                    to: "title".to_string(),
                    from: json!("name"),
                    transforms: Vec::new(),
                    fallback: None,
                }
            ],
            block_mappings: None,
        };

        let results = processor.process_documents_parallel(&documents, &mapping)?;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0]["title"], "Document 1");
        assert_eq!(results[1]["title"], "Document 2");
        assert_eq!(results[2]["title"], "Document 3");

        Ok(())
    }

    #[test]
    fn test_optimized_batch_processor() -> Result<()> {
        let batch_config = crate::batch::BatchConfig::default();
        let perf_config = PerformanceConfig {
            enable_memory_monitoring: false, // Disable to avoid hanging
            ..PerformanceConfig::default()
        };
        
        let mut processor = OptimizedBatchProcessor::new(batch_config, perf_config);
        
        let documents = vec![
            json!({"name": "Document 1"}),
            json!({"name": "Document 2"}),
        ];

        let mapping = crate::mapping::json_mapping::Mapping {
            source: "test".to_string(),
            target: "test".to_string(),
            collection: "test".to_string(),
            field_mappings: vec![
                crate::mapping::json_mapping::FieldMapping {
                    to: "title".to_string(),
                    from: json!("name"),
                    transforms: Vec::new(),
                    fallback: None,
                }
            ],
            block_mappings: None,
        };

        let results = processor.process_documents_optimized(&documents, &mapping)?;
        assert_eq!(results.len(), 2);

        Ok(())
    }
}
