require "minitest/autorun"

class StringTest < Minitest::Test
  def test_upcase
    assert_equal "ONCE", "once".upcase
  end
end
