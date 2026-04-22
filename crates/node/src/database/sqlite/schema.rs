// @generated automatically by Diesel CLI.

diesel::table! {
    notes (seq) {
        seq -> BigInt,
        id -> Binary,
        tag -> BigInt,
        header -> Binary,
        details -> Binary,
        created_at -> BigInt,
    }
}
