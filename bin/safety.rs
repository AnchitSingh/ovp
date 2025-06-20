// SAFETY AUDIT: Let's verify we haven't broken anything critical
use std::ptr;
use std::mem;

// Test all edge cases and potential safety violations
fn main() {
    println!("🔍 OVP Safety & Feature Audit");
    println!("═══════════════════════════════════");
    
    test_frame_integrity();
    test_memory_safety();
    test_protocol_features();
    test_edge_cases();
    test_unsafe_assumptions();
    
    println!("\n✅ All safety checks passed!");
}

fn test_frame_integrity() {
    println!("🔧 Frame Integrity Tests");
    println!("───────────────────────");
    
    // Test 1: Verify frames are byte-identical
    let targets = vec![1u64, 2u64, 3u64];
    let payload = b"Test payload with special chars: \x00\xFF\x42";
    
    let old_frame = build_ovp_frame_old(&targets, payload);
    
    let mut new_buffer = vec![0u8; 1500];
    let new_size = build_ovp_frame_optimized(&mut new_buffer, &targets, payload);
    let new_frame = &new_buffer[..new_size];
    
    assert_eq!(old_frame.len(), new_frame.len(), "Frame sizes don't match!");
    assert_eq!(old_frame, new_frame, "Frame contents don't match!");
    println!("✅ Frame integrity: IDENTICAL");
    
    // Test 2: Verify parsing gives same results
    let old_parsed = parse_ovp_frame_old(&old_frame, 2);
    let new_parsed = parse_ovp_frame_fast(&new_frame, 2);
    
    match (old_parsed, new_parsed) {
        (Some(old_payload), Some(new_payload)) => {
            assert_eq!(old_payload, new_payload, "Parsed payloads don't match!");
            println!("✅ Parse integrity: IDENTICAL");
        }
        (None, None) => println!("✅ Both correctly rejected frame"),
        _ => panic!("Parse results differ!"),
    }
}

fn test_memory_safety() {
    println!("\n🛡️  Memory Safety Tests");
    println!("────────────────────");
    
    // Test 1: Buffer bounds checking
    let targets = vec![1u64; 100]; // Large target list
    let payload = vec![0x42u8; 1000]; // Large payload
    
    let mut buffer = vec![0u8; 1500]; // Standard MTU
    let required_size = 12 + (targets.len() * 8) + payload.len();
    
    if required_size > buffer.len() {
        println!("✅ Large frame correctly exceeds buffer limit");
        // Test with smaller data
        let small_targets = vec![1u64, 2u64];
        let small_payload = b"small";
        let size = build_ovp_frame_optimized(&mut buffer, &small_targets, small_payload);
        assert!(size <= buffer.len(), "Buffer overflow!");
        println!("✅ Small frame fits in buffer");
    } else {
        let size = build_ovp_frame_optimized(&mut buffer, &targets, &payload);
        assert!(size <= buffer.len(), "Buffer overflow!");
        println!("✅ Large frame fits in buffer");
    }
    
    // Test 2: Alignment safety
    test_unaligned_access();
}

fn test_unaligned_access() {
    println!("⚡ Testing unaligned memory access safety");
    
    // Create intentionally misaligned data
    let mut test_data = vec![0u8; 32];
    
    // Write data at various alignments
    for offset in 0..8 {
        unsafe {
            let ptr = test_data.as_mut_ptr().add(offset);
            
            // Test u32 write/read
            ptr::write_unaligned(ptr as *mut u32, 0xDEADBEEF);
            let read_u32 = ptr::read_unaligned(ptr as *const u32);
            assert_eq!(read_u32, 0xDEADBEEF, "u32 unaligned access failed at offset {}", offset);
            
            // Test u64 write/read
            if offset + 8 <= test_data.len() {
                ptr::write_unaligned(ptr as *mut u64, 0xCAFEBABEDEADBEEF);
                let read_u64 = ptr::read_unaligned(ptr as *const u64);
                assert_eq!(read_u64, 0xCAFEBABEDEADBEEF, "u64 unaligned access failed at offset {}", offset);
            }
        }
    }
    println!("✅ Unaligned access works at all offsets");
}

fn test_protocol_features() {
    println!("\n🎯 Protocol Features Test");
    println!("─────────────────────");
    
    // Test 1: Broadcast messages (no targets)
    test_broadcast_message();
    
    // Test 2: Targeted messages
    test_targeted_message();
    
    // Test 3: Multi-target messages
    test_multi_target_message();
    
    // Test 4: Empty payload
    test_empty_payload();
    
    // Test 5: Large payload
    test_large_payload();
}

fn test_broadcast_message() {
    let targets: Vec<u64> = vec![]; // Broadcast
    let payload = b"Broadcast to all drones!";
    
    let mut buffer = vec![0u8; 1500];
    let size = build_ovp_frame_optimized(&mut buffer, &targets, payload);
    let frame = &buffer[..size];
    
    // Any drone should receive broadcast
    let received = parse_ovp_frame_fast(frame, 999);
    assert!(received.is_some(), "Broadcast not received!");
    assert_eq!(received.unwrap(), payload, "Broadcast payload corrupted!");
    println!("✅ Broadcast messages work");
}

fn test_targeted_message() {
    let targets = vec![42u64];
    let payload = b"Message for drone 42";
    
    let mut buffer = vec![0u8; 1500];
    let size = build_ovp_frame_optimized(&mut buffer, &targets, payload);
    let frame = &buffer[..size];
    
    // Target drone should receive
    let received = parse_ovp_frame_fast(frame, 42);
    assert!(received.is_some(), "Target didn't receive message!");
    
    // Non-target should not receive
    let not_received = parse_ovp_frame_fast(frame, 43);
    assert!(not_received.is_none(), "Non-target received message!");
    println!("✅ Targeted messages work");
}

fn test_multi_target_message() {
    let targets = vec![1u64, 5u64, 10u64, 42u64];
    let payload = b"Group message";
    
    let mut buffer = vec![0u8; 1500];
    let size = build_ovp_frame_optimized(&mut buffer, &targets, payload);
    let frame = &buffer[..size];
    
    // All targets should receive
    for &target in &targets {
        let received = parse_ovp_frame_fast(frame, target);
        assert!(received.is_some(), "Target {} didn't receive group message!", target);
    }
    
    // Non-target should not receive
    let not_received = parse_ovp_frame_fast(frame, 999);
    assert!(not_received.is_none(), "Non-target received group message!");
    println!("✅ Multi-target messages work");
}

fn test_empty_payload() {
    let targets = vec![1u64];
    let payload: &[u8] = &[];
    
    let mut buffer = vec![0u8; 1500];
    let size = build_ovp_frame_optimized(&mut buffer, &targets, payload);
    let frame = &buffer[..size];
    
    let received = parse_ovp_frame_fast(frame, 1);
    assert!(received.is_some(), "Empty payload not received!");
    assert_eq!(received.unwrap().len(), 0, "Empty payload has content!");
    println!("✅ Empty payloads work");
}

fn test_large_payload() {
    let targets = vec![1u64];
    let payload = vec![0x55u8; 1200]; // Large but fits in MTU
    
    let mut buffer = vec![0u8; 1500];
    let size = build_ovp_frame_optimized(&mut buffer, &targets, &payload);
    let frame = &buffer[..size];
    
    let received = parse_ovp_frame_fast(frame, 1);
    assert!(received.is_some(), "Large payload not received!");
    assert_eq!(received.unwrap(), payload, "Large payload corrupted!");
    println!("✅ Large payloads work");
}

fn test_edge_cases() {
    println!("\n⚠️  Edge Cases Test");
    println!("─────────────────");
    
    // Test 1: Malformed frames
    test_malformed_frames();
    
    // Test 2: Buffer edge cases
    test_buffer_edges();
    
    // Test 3: Numeric edge cases
    test_numeric_edges();
}

fn test_malformed_frames() {
    // Too short frame
    let short_frame = vec![0u8; 5];
    let result = parse_ovp_frame_fast(&short_frame, 1);
    assert!(result.is_none(), "Short frame should be rejected!");
    
    // Wrong magic
    let mut wrong_magic = vec![0u8; 20];
    unsafe {
        ptr::write_unaligned(wrong_magic.as_mut_ptr() as *mut u32, 0x12345678u32.to_le());
    }
    let result = parse_ovp_frame_fast(&wrong_magic, 1);
    assert!(result.is_none(), "Wrong magic should be rejected!");
    
    // Inconsistent lengths
    let mut bad_length = vec![0u8; 50];
    unsafe {
        let ptr = bad_length.as_mut_ptr();
        ptr::write_unaligned(ptr as *mut u32, 0xDEADBEEFu32.to_le()); // magic
        ptr::write_unaligned(ptr.add(4) as *mut u32, 1u32.to_le()); // 1 target
        ptr::write_unaligned(ptr.add(8) as *mut u32, 100u32.to_le()); // 100 byte payload (but frame too short)
    }
    let result = parse_ovp_frame_fast(&bad_length, 1);
    assert!(result.is_none(), "Inconsistent length should be rejected!");
    
    println!("✅ Malformed frames correctly rejected");
}

fn test_buffer_edges() {
    // Test exactly at buffer boundary
    let targets = vec![1u64];
    let mut payload = Vec::new();
    payload.resize(1500 - 12 - 8, 0x42); // Fill exactly to buffer limit
    
    let mut buffer = vec![0u8; 1500];
    let size = build_ovp_frame_optimized(&mut buffer, &targets, &payload);
    assert_eq!(size, 1500, "Should use entire buffer");
    
    let received = parse_ovp_frame_fast(&buffer[..size], 1);
    assert!(received.is_some(), "Buffer-edge frame should parse!");
    println!("✅ Buffer edge cases work");
}

fn test_numeric_edges() {
    // Test with maximum drone IDs
    let targets = vec![u64::MAX];
    let payload = b"Max drone ID test";
    
    let mut buffer = vec![0u8; 1500];
    let size = build_ovp_frame_optimized(&mut buffer, &targets, payload);
    let frame = &buffer[..size];
    
    let received = parse_ovp_frame_fast(frame, u64::MAX);
    assert!(received.is_some(), "Max drone ID should work!");
    println!("✅ Numeric edge cases work");
}

fn test_unsafe_assumptions() {
    println!("\n🔥 Unsafe Code Assumptions");
    println!("─────────────────────────");
    
    // Verify our unsafe assumptions are valid
    println!("Architecture: {}", std::env::consts::ARCH);
    println!("Endianness: little = {}", cfg!(target_endian = "little"));
    
    // Test that our unaligned access assumptions are correct
    assert!(mem::size_of::<u32>() == 4, "u32 not 4 bytes!");
    assert!(mem::size_of::<u64>() == 8, "u64 not 8 bytes!");
    
    // Test that our magic constant fits
    assert_eq!(0xDEADBEEFu32.to_le_bytes().len(), 4, "Magic constant wrong size!");
    
    println!("✅ All unsafe assumptions verified");
}

// Include the original and optimized implementations for testing
fn build_ovp_frame_old(targets: &[u64], payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&0xDEADBEEFu32.to_le_bytes());
    frame.extend_from_slice(&(targets.len() as u32).to_le_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    for &target in targets {
        frame.extend_from_slice(&target.to_le_bytes());
    }
    frame.extend_from_slice(payload);
    frame
}

fn parse_ovp_frame_old(frame: &[u8], my_id: u64) -> Option<Vec<u8>> {
    if frame.len() < 12 { return None; }
    let magic = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
    if magic != 0xDEADBEEF { return None; }
    let target_count = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
    let payload_len = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]) as usize;
    let targets_start = 12;
    let targets_end = targets_start + (target_count * 8);
    let payload_start = targets_end;
    let payload_end = payload_start + payload_len;
    if frame.len() < payload_end { return None; }
    if target_count > 0 {
        let mut found = false;
        for i in 0..target_count {
            let offset = targets_start + (i * 8);
            let target_id = u64::from_le_bytes([
                frame[offset], frame[offset+1], frame[offset+2], frame[offset+3],
                frame[offset+4], frame[offset+5], frame[offset+6], frame[offset+7]
            ]);
            if target_id == my_id { found = true; break; }
        }
        if !found { return None; }
    }
    Some(frame[payload_start..payload_end].to_vec())
}

fn build_ovp_frame_optimized(buffer: &mut [u8], targets: &[u64], payload: &[u8]) -> usize {
    let header_size = 12;
    let targets_size = targets.len() * 8;
    let total_size = header_size + targets_size + payload.len();
    
    if total_size > buffer.len() {
        panic!("Frame too large for buffer!");
    }
    
    unsafe {
        let buf = buffer.as_mut_ptr();
        ptr::write_unaligned(buf as *mut u32, 0xDEADBEEFu32.to_le());
        ptr::write_unaligned(buf.add(4) as *mut u32, (targets.len() as u32).to_le());
        ptr::write_unaligned(buf.add(8) as *mut u32, (payload.len() as u32).to_le());
        
        let mut offset = 12;
        for &target in targets {
            ptr::write_unaligned(buf.add(offset) as *mut u64, target.to_le());
            offset += 8;
        }
        
        ptr::copy_nonoverlapping(payload.as_ptr(), buf.add(offset), payload.len());
    }
    
    total_size
}

fn parse_ovp_frame_fast(frame: &[u8], my_id: u64) -> Option<&[u8]> {
    if frame.len() < 12 { return None; }
    
    unsafe {
        let magic = ptr::read_unaligned(frame.as_ptr() as *const u32);
        if u32::from_le(magic) != 0xDEADBEEF { return None; }
        
        let target_count = u32::from_le(ptr::read_unaligned(frame.as_ptr().add(4) as *const u32)) as usize;
        let payload_len = u32::from_le(ptr::read_unaligned(frame.as_ptr().add(8) as *const u32)) as usize;
        
        let targets_start = 12;
        let targets_end = targets_start + (target_count * 8);
        let payload_start = targets_end;
        let payload_end = payload_start + payload_len;
        
        if frame.len() < payload_end { return None; }
        
        if target_count == 0 {
            return Some(&frame[payload_start..payload_end]);
        }
        
        let targets_ptr = frame.as_ptr().add(targets_start) as *const u64;
        for i in 0..target_count {
            let target_id = u64::from_le(ptr::read_unaligned(targets_ptr.add(i)));
            if target_id == my_id {
                return Some(&frame[payload_start..payload_end]);
            }
        }
        
        None
    }
}