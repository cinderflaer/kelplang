// A `datac` as the element type: the instantiation is emitted together with
// that struct, so its typedef and its drop function are already defined, and
// each element is freed by the struct's own generated drop.
datac Card(val rank: i32, val suit: String)

datac Deck(val cards: List<Card>)

fun main() {
    var cards: List<Card> = List.empty()
    cards.push(Card(rank = 1, suit = String.from_str("hearts")))
    cards.push(Card(rank = 2, suit = String.from_str("spades")))
    var deck = Deck(cards = cards)
    print(deck.cards.len())
    print(deck.cards.get(1).suit)
    val top = deck.cards.pop()
    print(top.rank)
    print(top.suit)
}
