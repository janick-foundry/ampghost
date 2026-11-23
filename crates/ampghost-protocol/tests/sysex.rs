use std::error::Error;

use ampghost_protocol::{
    Address, Dt1, Incoming, KATANA_50_MK2, PatchImage, PatchOffset, ProtocolError, Rq1, Size,
    SysexHeader, checksum, split_messages,
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn checksum_matches_known_frames() {
    assert_eq!(checksum(&[0x60, 0x00, 0x12, 0x14, 0x01]), 0x79);
    assert_eq!(checksum(&[0x7F, 0x00, 0x01, 0x04, 0x00, 0x05]), 0x77);
    assert_eq!(checksum(&[0x7F, 0x00, 0x00, 0x01, 0x01]), 0x7F);
    assert_eq!(checksum(&[0x7F, 0x00, 0x00, 0x01, 0x00]), 0x00);
    assert_eq!(checksum(&[]), 0x00);
}

#[test]
fn rq1_for_temp_encodes_known_frame() {
    let request = Rq1::new(
        Address::from_parts(0x60, 0x00, 0x00, 0x00),
        Size::from_parts(0x00, 0x00, 0x0F, 0x48),
    );
    assert_eq!(
        request.encode(&KATANA_50_MK2.sysex),
        [
            0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x11, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x0F, 0x48, 0x49, 0xF7
        ]
    );
}

#[test]
fn dt1_round_trips_through_decode() -> TestResult {
    let frame = [
        0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x12, 0x60, 0x00, 0x12, 0x14, 0x01, 0x79, 0xF7,
    ];
    let packet = Dt1::new(Address::from_parts(0x60, 0x00, 0x12, 0x14), [0x01])?;
    assert_eq!(packet.encode(&KATANA_50_MK2.sysex), frame);
    assert_eq!(KATANA_50_MK2.decode(&frame)?, Incoming::Dt1(packet));
    Ok(())
}

#[test]
fn decode_rejects_bad_checksum_and_frames() {
    let bad = [
        0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x12, 0x60, 0x00, 0x12, 0x14, 0x01, 0x78, 0xF7,
    ];
    assert!(matches!(
        KATANA_50_MK2.decode(&bad),
        Err(ProtocolError::Checksum {
            expected: 0x79,
            actual: 0x78,
            ..
        })
    ));
    assert!(matches!(
        KATANA_50_MK2.decode(&[0xF0, 0x41]),
        Err(ProtocolError::BadFrame)
    ));
    assert!(matches!(
        Dt1::new(Address::from_parts(0x60, 0, 0, 0), [0x80]),
        Err(ProtocolError::NotSevenBit { .. })
    ));
    assert!(matches!(
        Dt1::new(Address::from_parts(0x60, 0, 0, 0), []),
        Err(ProtocolError::EmptyData)
    ));
}

#[test]
fn split_messages_ignores_noise() {
    let stream = [0x00, 0xF0, 0x01, 0xF7, 0xFE, 0xF0, 0x02, 0xF7, 0xF0, 0x03];
    assert_eq!(
        split_messages(&stream),
        vec![&[0xF0, 0x01, 0xF7][..], &[0xF0, 0x02, 0xF7][..]]
    );
}

#[test]
fn check_write_allows_temp_and_editor_mode_only() -> TestResult {
    let profile = &KATANA_50_MK2;
    profile.check_write(&Dt1::new(
        Address::from_parts(0x60, 0x00, 0x00, 0x00),
        [0x41],
    )?)?;
    profile.check_write(&profile.editor_mode_on()?)?;
    for (address, data) in [
        (
            Address::from_parts(0x7F, 0x00, 0x01, 0x04),
            vec![0x00, 0x05],
        ),
        (
            Address::from_parts(0x7F, 0x00, 0x00, 0x01),
            vec![0x01, 0x00],
        ),
        (Address::from_parts(0x00, 0x01, 0x00, 0x00), vec![0x05]),
        (Address::from_parts(0x10, 0x01, 0x00, 0x00), vec![0x00]),
        (Address::from_parts(0x60, 0x00, 0x07, 0x44), vec![0x00]),
        (
            Address::from_parts(0x60, 0x00, 0x07, 0x43),
            vec![0x00, 0x00],
        ),
        (Address::from_parts(0x60, 0x00, 0x0F, 0x48), vec![0x00]),
    ] {
        let packet = Dt1::new(address, data)?;
        assert!(
            matches!(
                profile.check_write(&packet),
                Err(ProtocolError::WriteForbidden { .. })
            ),
            "{packet}"
        );
    }
    Ok(())
}

#[test]
fn full_write_plan_orders_type_selects_first_and_stays_in_temp() -> TestResult {
    let profile = &KATANA_50_MK2;
    let image = PatchImage::from_data(
        &profile.patch_layout,
        vec![0; profile.patch_layout.data_len()],
    )?;
    let plan = profile.full_write_plan(&image)?;
    let total: usize = plan.iter().map(|packet| packet.data().len()).sum();
    assert_eq!(total, profile.patch_layout.data_len());
    assert!(
        plan.iter()
            .all(|packet| packet.data().len() <= profile.write_order.max_packet)
    );
    let first = plan.first().map(Dt1::address);
    assert_eq!(first, Some(Address::from_parts(0x60, 0x00, 0x06, 0x24)));
    for packet in &plan {
        profile.check_write(packet)?;
    }
    let partial = profile.partial_write_plan(
        &image,
        &[PatchOffset::new(0x00, 0x22), PatchOffset::new(0x00, 0x23)],
    )?;
    assert_eq!(partial.len(), 1);
    Ok(())
}

#[test]
fn editor_mode_frames_match_boss_tone_studio() -> TestResult {
    let profile = &KATANA_50_MK2;
    assert_eq!(
        profile.editor_mode_off()?.encode(&KATANA_50_MK2.sysex),
        [
            0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x12, 0x7F, 0x00, 0x00, 0x01, 0x00, 0x00,
            0xF7
        ]
    );
    assert_eq!(
        profile.editor_mode_on()?.encode(&KATANA_50_MK2.sysex),
        [
            0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x12, 0x7F, 0x00, 0x00, 0x01, 0x01, 0x7F,
            0xF7
        ]
    );
    Ok(())
}

#[test]
fn check_write_allows_editor_mode_on_and_off_only() -> TestResult {
    let profile = &KATANA_50_MK2;
    profile.check_write(&profile.editor_mode_on()?)?;
    profile.check_write(&profile.editor_mode_off()?)?;
    let editor = Address::from_parts(0x7F, 0x00, 0x00, 0x01);
    for data in [
        vec![0x02],
        vec![0x7F],
        vec![0x00, 0x00],
        vec![0x01, 0x01],
        vec![0x00, 0x01, 0x00],
    ] {
        let packet = Dt1::new(editor, data)?;
        assert!(
            matches!(
                profile.check_write(&packet),
                Err(ProtocolError::WriteForbidden { .. })
            ),
            "{packet}"
        );
    }
    for address in [
        Address::from_parts(0x7F, 0x00, 0x00, 0x00),
        Address::from_parts(0x7F, 0x00, 0x00, 0x02),
    ] {
        let packet = Dt1::new(address, [0x00])?;
        assert!(
            matches!(
                profile.check_write(&packet),
                Err(ProtocolError::WriteForbidden { .. })
            ),
            "{packet}"
        );
    }
    Ok(())
}

const MK2_DT1_PREFIX: [u8; 8] = [0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x12];

fn mk2_dt1_frame(packet: &Dt1) -> Vec<u8> {
    let mut body = packet.address().bytes().to_vec();
    body.extend_from_slice(packet.data());
    let mut frame = MK2_DT1_PREFIX.to_vec();
    frame.extend_from_slice(&body);
    frame.push(checksum(&body));
    frame.push(0xF7);
    frame
}

#[test]
fn mk2_header_and_requests_stay_byte_identical() {
    let profile = &KATANA_50_MK2;
    assert_eq!(
        profile.sysex.bytes(),
        [0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33]
    );
    assert_eq!(
        profile.encode_request(&profile.read_current_channel_request()),
        [
            0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x11, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x02, 0x7D, 0xF7
        ]
    );
    assert_eq!(
        profile.encode_request(&profile.read_program_change_map_request()),
        [
            0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x11, 0x00, 0x02, 0x00, 0x0C, 0x00, 0x00,
            0x00, 0x09, 0x69, 0xF7
        ]
    );
    assert_eq!(
        profile.encode_request(&profile.read_temp_request()),
        [
            0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x11, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x0F, 0x48, 0x49, 0xF7
        ]
    );
}

#[test]
fn mk2_write_frames_stay_byte_identical() -> TestResult {
    let profile = &KATANA_50_MK2;
    let data = (0..profile.patch_layout.data_len())
        .map(|index| u8::try_from(index % 0x80))
        .collect::<Result<Vec<_>, _>>()?;
    let image = PatchImage::from_data(&profile.patch_layout, data)?;
    let mut packets = profile.full_write_plan(&image)?;
    packets.push(profile.editor_mode_on()?);
    packets.push(profile.editor_mode_off()?);
    for packet in &packets {
        let frame = profile.encode_write(packet);
        assert_eq!(frame, mk2_dt1_frame(packet), "{packet}");
        assert_eq!(profile.decode(&frame)?, Incoming::Dt1(packet.clone()));
    }
    Ok(())
}

#[test]
fn headers_of_other_generations_stay_apart() -> TestResult {
    let gen3 = SysexHeader::new(0x10, &[0x01, 0x05, 0x07]);
    let request = Rq1::new(
        Address::from_parts(0x20, 0x00, 0x00, 0x00),
        Size::from_parts(0x00, 0x00, 0x00, 0x01),
    );
    assert_eq!(
        request.encode(&gen3),
        [
            0xF0, 0x41, 0x10, 0x01, 0x05, 0x07, 0x11, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x01, 0x5F, 0xF7
        ]
    );
    let packet = Dt1::new(Address::from_parts(0x20, 0x00, 0x00, 0x00), [0x05])?;
    let gen3_frame = packet.encode(&gen3);
    assert_eq!(gen3.decode(&gen3_frame)?, Incoming::Dt1(packet.clone()));
    assert_eq!(
        KATANA_50_MK2.decode(&gen3_frame)?,
        Incoming::Other(gen3_frame.clone())
    );
    let mk2_frame = KATANA_50_MK2.encode_write(&packet);
    assert_eq!(gen3.decode(&mk2_frame)?, Incoming::Other(mk2_frame.clone()));
    Ok(())
}
