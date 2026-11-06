"""How a fused loop handler is cut into one piece per record part (bgdeobf/classify.py, split_by_part).
The pieces are plain stand-ins here: `parts_of` says which record parts a statement uses."""
from bgdeobf.classify import split_by_part


def parts(table):
    return lambda s: table.get(s, set())


def test_a_statement_without_a_part_goes_with_the_part_before_it():
    assert split_by_part(['a', 'x', 'b'], parts({'a': {0}, 'b': {1}})) == [[0, ['a', 'x']], [1, ['b']]]


def test_statements_without_a_part_at_the_start_go_with_the_next_part():
    # `local f; f = K(B); R[A] = f` : the first statement only declares the temporary the second one fills
    got = split_by_part(['decl', 'a', 'a2', 'b'], parts({'a': {0}, 'a2': {0}, 'b': {1}}))
    assert got == [[0, ['decl', 'a', 'a2']], [1, ['b']]]


def test_consecutive_statements_of_one_part_stay_together():
    assert split_by_part(['a', 'a2', 'b', 'b2'], parts({'a': {0}, 'a2': {0}, 'b': {1}, 'b2': {1}})) \
        == [[0, ['a', 'a2']], [1, ['b', 'b2']]]


def test_a_statement_that_uses_two_parts_cannot_be_cut():
    assert split_by_part(['a', 'ab'], parts({'a': {0}, 'ab': {0, 1}})) is None


def test_nothing_to_attach_to_cannot_be_cut():
    assert split_by_part(['x', 'y'], parts({})) is None


def test_no_statements_give_no_pieces():
    assert split_by_part([], parts({})) == []
