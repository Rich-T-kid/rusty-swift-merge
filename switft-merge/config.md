# all keys must exist or else an error will be thrown

### _ramMaxSize_

1. **this is the maximum size the memtable will grow to before being written to disk**
2. **the value provided must be the number of bytes**
3. **(min_value,max_value) -> (1KB,2GB)**
4. should fit in a uint32

### _ramMaxTime_

1. **represents the max amount of time data will be held in memtable before being flushed to disk. In seconds**
2. **this timer is set by every flush,weather triggered by _ramMaxTime_ or not**
3. **(min,max) -> (10,10080) [10 seconds ,168 hours]**
4. **should fit in u16**

### _targetChunks_

- level_n_max_size = ramMaxSize × (targetChunks ^ (n+1))

1. **represents the target number of SSTables (chunks) each level should aim to maintain**

2. **this is not a strict limit, but a structural goal used to determine when compaction should occur**

3. **when the number of chunks in a level exceeds this value, compaction is triggered**

4. **level 0 may temporarily contain fewer chunks depending on flush frequency**

5. **(min,max) -> (2,128)**

6. **should fit in u8**

### _compactionCheckIntervalSeconds_

1. **represents in seconds the time inbetween when a background thread checks for _chunksPerLevel_ and _compactionRate_ and updates the lsm-tree accordingly**
2. **(min,max) -> (1,14400) [one second,4 hours]**
3. **should fit in u16**

### _walEnabled_

1. **if set to true all writes will first be written to Write Ahead Logs. this ensures in case of crash data can be recovered**
2. **bool**

### _bloomFalsePositiveRate_

1. **represents the acceptable false positive rate for bloom filters used in ss-tables**
2. **lower values reduce false positives but increase memory usage**
3. **(min,max) -> (0.001,0.1) [0.1% to 10%]**
4. **should be a float64**

### _maxCompactionThreads_

1. **represents the maximum number of threads that can be used for compaction operations**
2. **higher values speed up compaction but use more system resources**
3. **(min,max) -> (1,system_thread_max)**
4. **should fit in u8**

### _localDisk_

1. **represents wheather to store SS-tables on local file system or on cloud storage**
2. **bool**
