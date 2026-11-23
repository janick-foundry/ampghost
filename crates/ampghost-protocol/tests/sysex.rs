use std::error::Error;

use ampghost_protocol::{
    Address, Dt1, Incoming, KATANA_50_MK2, PatchImage, PatchOffset, ProtocolError, Rq1, Size,
    checksum, decode, split_messages,
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
        request.encode(),
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
    assert_eq!(packet.encode(), frame);
    assert_eq!(decode(&frame)?, Incoming::Dt1(packet));
    Ok(())
}

#[test]
fn decode_rejects_bad_checksum_and_frames() {
    let bad = [
        0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x12, 0x60, 0x00, 0x12, 0x14, 0x01, 0x78, 0xF7,
    ];
    assert!(matches!(
        decode(&bad),
        Err(ProtocolError::Checksum {
            expected: 0x79,
            actual: 0x78,
            ..
        })
    ));
    assert!(matches!(
        decode(&[0xF0, 0x41]),
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
        profile.editor_mode_off()?.encode(),
        [
            0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33, 0x12, 0x7F, 0x00, 0x00, 0x01, 0x00, 0x00,
            0xF7
        ]
    );
    assert_eq!(
        profile.editor_mode_on()?.encode(),
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
