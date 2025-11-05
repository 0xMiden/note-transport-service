mod common;

use anyhow::Result;

use self::common::*;

#[tokio::test]
async fn test_basic_exchange() -> Result<()> {
    let port = 9627;
    let handle = spawn_test_server(port).await;

    let mut client0 = mock_client(port).await;
    let mut client1 = mock_client(port).await;

    let tag = client1.address.to_note_tag();

    let note = mock_note_p2id_with_addresses(&client0.address, &client1.address);

    client0.send_note(note, &client1.address).await?;
    let (notes, _cursor) = client1.fetch_notes(&[tag], 0).await?;

    assert_eq!(notes.len(), 1);
    let header = notes[0].header();
    let rx_tag = header.metadata().tag();
    assert_eq!(rx_tag, tag);

    handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_basic_pagination() -> Result<()> {
    let port = 9628;
    let handle = spawn_test_server(port).await;

    let mut client0 = mock_client(port).await;
    let mut client1 = mock_client(port).await;

    let tag = client1.address.to_note_tag();

    let note_a = mock_note_p2id_with_addresses(&client0.address, &client1.address);
    let note_b = mock_note_p2id_with_addresses(&client0.address, &client1.address);

    client0.send_note(note_a, &client1.address).await?;
    let (notes, cursor_a) = client1.fetch_notes(&[tag], 0).await?;
    assert_eq!(notes.len(), 1);

    client0.send_note(note_b, &client1.address).await?;
    // no pagination (fetch all)
    let (notes, _) = client1.fetch_notes(&[tag], 0).await?;
    assert_eq!(notes.len(), 2);

    // pagination, cursor after first note
    let (notes, _) = client1.fetch_notes(&[tag], cursor_a).await?;
    assert_eq!(notes.len(), 1);

    handle.abort();
    Ok(())
}
