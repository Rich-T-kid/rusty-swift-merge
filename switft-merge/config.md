# json doesnt allow comments so this will be my notes

### _ramMaxSize_

1. **this is the maximum size the memtable will grow to before being written to disk**
2. **the value provided must be the number of bytes**
3. **(min_value,max_value) -> (16kb,2GB)**
4. should fit in a uint32

### _ramMaxTime_

1. **represents the max amount of time data will be held in memtable before being flushed to disk. In minutes**
2. **this timer is set by every flush,weather triggered by _ramMaxTime_ or not**
3. **(min,max) -> (60,10080) [1 hour,7 days]**
4. **should fit in u16**

### _chunksPerLevel_

1. **represents the number of ss-tables/ memtable chunks that should exist before compaction occures**
2. **(min_value,max_value) -> (2,255)**
3. **should fit in u8**

### _compactionRate_

1. **represents the numbers of previous ss-tables/ memtable chunks that will compacted into a new singler chunk**
2. **if there arent enough chunks for the new,compacted chunk it reads the remaining chunks and combines those**
3. **should fit in u8**
4. **(min,max) -> (2,8)**

### _workerThreadRefresh_

1. **represents in minutes the time inbetween when a background thread checks for _chunksPerLevel_ and _compactionRate_ and updates the lsm-tree accordingly**
2. **(min,max) -> (1,240) [one minute,4 hours]**
3. **should fit in u8**
