// @generated automatically by Diesel CLI.

diesel::table! {
    notes (seq) {
        seq -> BigInt,
        id -> Binary,
        tag -> BigInt,
        header -> Binary,
        details -> Binary,
        created_at -> BigInt,
        commitment_block_num -> Nullable<Integer>,
        note_metadata -> Nullable<Binary>,
    }
}
