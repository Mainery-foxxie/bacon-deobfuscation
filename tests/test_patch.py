"""The last statement of a protected script, which the patcher replaces by a call of the dumper.  The scripts here are
tiny stand-ins: only the syntax tree is read, there is no Bacon Guard analysis."""
import pytest

from bgdeobf.loader import LayoutError, Loader
from bgdeobf.patch import tail_replacement


class Bare(Loader):
    def analyse(self):
        pass


def tail(text):
    L = Bare(text)
    last, new = tail_replacement(L)
    assert L.src.slice(last).startswith('return')
    return new


@pytest.mark.parametrize('text,expected', [
    # runner function that builds the module and calls the result: the building call moves into the dumper
    ('local module = make()\nlocal function run(...) return module(1, 2)(...) end\nreturn run(...)',
     '__DUMPER(module(1, 2))'),
    # runner that is not a function written out here: the dumper is called as it is
    ('local run = make()\nreturn run(...)', '__DUMPER()'),
    # newer builds (program stored in compressed pages): the module is called right in the last statement
    ('local module = make()\nlocal host = {}\nlocal env = getfenv()\nreturn module(host, env)(...)',
     '__DUMPER(module(host, env))'),
    ('local module = make()\nlocal a, b = 1, 2\nreturn module( a , b )(...)', '__DUMPER(module( a , b ))'),
    # the results of that call are kept in a table first, and the first one is called: the module has run by then
    ('local module = make()\nlocal host = {}\nlocal env = getfenv()\nlocal results = {module(host, env)}\n'
     'return results[1](...)', '__DUMPER()'),
])
def test_known_tail_shapes(text, expected):
    assert tail(text) == expected


@pytest.mark.parametrize('text,message', [
    ('local x = 1\nreturn 5', 'tail not recognised'),
    ('local x = 1\nprint(x)', 'tail not recognised'),
    ('local x = 1\nreturn print(x)', 'tail not recognised'),
    ('local t = {}\nreturn t.run()(...)', 'tail not recognised'),
    ('local run = make()\nreturn run(...), 2', 'tail not recognised'),
    ('local function run(...) print(1) end\nreturn run(...)', 'runner not recognised'),
    ('local function t() end\nreturn t[1](...)', 'runner not recognised'),
    ('local i = 1\nlocal t = {}\nreturn t[i](...)', 'tail not recognised'),
    ('return results[1](...)', 'tail not recognised'),
])
def test_unknown_tail_shapes_are_refused(text, message):
    with pytest.raises(LayoutError) as e:
        tail(text)
    assert message in str(e.value)
