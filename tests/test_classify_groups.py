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


# ---- record parts read inside the loop (Bacon Guard v0.42, build 814ce50c5703) ---------------------------------------------
# A fused `for ... in` handler of that build reads the part of its body inside the loop, in a block of its own, where older
# builds read every part in front of the loop.  The handler below is made up; it has the same shape.
from bgdeobf.astutil import lkey, parse_ast, walk
from bgdeobf.classify import hoist_loop_reads, register_read

INNER = '''
local function handler(rec, regs, dec, nxt, top)
    local p0 = rec[11]
    local a = dec(p0)
    local p2 = rec[13]
    local b, c = dec(p2)
    while true do
        regs[b] = regs[b] + 1
        if regs[b] > c then return nxt, top end
        do
            local p1 = rec[12]
            local x, y = dec(p1)
            do regs[y] = regs[x] end
        end
    end
end
'''

FRONT = '''
local function handler(rec, regs, dec, nxt, top)
    local p0 = rec[11]
    local a = dec(p0)
    local p1 = rec[12]
    local x, y = dec(p1)
    local p2 = rec[13]
    local b, c = dec(p2)
    while true do
        regs[b] = regs[b] + 1
        if regs[b] > c then return nxt, top end
        do regs[y] = regs[x] end
    end
end
'''


def read_parts(source):
    """-> (loop body after the hoisting, var_part, part_of, reads), as classify_loop_ded sets them up"""
    fn = parse_ast(source)['body'][0]['func']
    body = fn['body']['body']
    rec_loc = fn['args'][0]['location']
    var_part, part_of, reads = {}, {}, {}
    for s_ in body[:-1]:
        register_read(s_, rec_loc, var_part, part_of, reads)
    loop_body = hoist_loop_reads(body[-1]['body']['body'], rec_loc, var_part, part_of, reads)
    return loop_body, var_part, part_of, reads


def groups(loop_body, part_of):
    def parts_in(s_):
        return set(part_of[lkey(n['local'])] for n, p in walk(s_) if n['type'] == 'AstExprLocal' and lkey(n['local']) in part_of)
    return split_by_part(loop_body, parts_in)


def test_a_part_read_inside_the_loop_is_registered_like_the_ones_in_front_of_it():
    loop_body, var_part, part_of, reads = read_parts(INNER)
    assert sorted(reads) == [0, 1, 2]
    assert len(reads[1]) == 2                      # `local p1 = rec[12]` and `local x, y = dec(p1)`


def test_the_block_that_held_the_read_is_opened_up_so_the_body_is_a_piece_of_its_own_part():
    loop_body, var_part, part_of, reads = read_parts(INNER)
    got = groups(loop_body, part_of)
    assert [g[0] for g in got] == [2, 1]           # loop control (part 2), then the body (part 1)
    assert [len(g[1]) for g in got] == [2, 1]
    assert any(n['type'] == 'AstStatReturn' for s_ in got[0][1] for n, p in walk(s_))     # the control is the one that returns


def test_without_the_hoisting_the_body_would_be_taken_for_a_part_of_the_loop_control():
    fn = parse_ast(INNER)['body'][0]['func']
    body = fn['body']['body']
    var_part, part_of, reads = {}, {}, {}
    for s_ in body[:-1]:
        register_read(s_, fn['args'][0]['location'], var_part, part_of, reads)
    got = groups(body[-1]['body']['body'], part_of)
    assert [g[0] for g in got] == [2]              # one piece: the control with the block attached (what failed before)


def test_reads_in_front_of_the_loop_are_left_as_they_were():
    loop_body, var_part, part_of, reads = read_parts(FRONT)
    assert sorted(reads) == [0, 1, 2]
    assert [g[0] for g in groups(loop_body, part_of)] == [2, 1]


def test_a_block_that_reads_no_part_is_kept_as_a_block():
    loop_body, var_part, part_of, reads = read_parts(FRONT)
    assert [s_['type'] for s_ in loop_body] == ['AstStatAssign', 'AstStatIf', 'AstStatBlock']
